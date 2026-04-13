import {
  FunctionDeclaration,
  GoogleGenAI,
  Tool,
  Type,
  type Content,
  type GenerateContentConfig,
  type Part,
  type Schema,
} from '@google/genai';
import type {
  LLMMessage,
  LLMProvider,
  LLMResponse,
  LLMToolCall,
  LLMToolDefinition,
  LLMTokenUsage,
} from '../types';

/** Usage metadata returned with cloud media helpers (image / audio / video). */
type GeminiMediaUsage = LLMTokenUsage & { count?: number; duration?: number };

/** Narrow view of long-running video operations from the GenAI JS SDK. */
type GenerateVideosOperation = {
  done?: boolean;
  response?: {
    generatedVideos?: Array<{
      durationSeconds?: number;
      video?: { uri?: string };
    }>;
  };
};

type GenaiModelsWithVideo = {
  generateVideos: (payload: unknown) => Promise<GenerateVideosOperation>;
};

type GenaiClientWithVideoOps = {
  operations: {
    getVideosOperation: (params: {
      operation: GenerateVideosOperation;
    }) => Promise<GenerateVideosOperation>;
  };
};

type VideoGenerationOptions = {
  resolution?: '720p' | '1080p' | '4k';
  aspectRatio?: '16:9' | '9:16';
  durationSeconds?: 4 | 6 | 8;
};

type VideoConfigPayload = {
  resolution: string;
  aspectRatio: string;
  durationSeconds: number;
  sampleCount: number;
  referenceImages?: Array<Record<string, unknown>>;
};
import { DEFAULT_MODELS } from '../constants';
import { calculateTokensForCost } from '../pricing';


export class GeminiProvider implements LLMProvider {
  private client: GoogleGenAI;

  constructor(private apiKey: string) {
    this.client = new GoogleGenAI({ apiKey });
  }

  async generateCompletion(
    messages: LLMMessage[],
    tools?: LLMToolDefinition[],
    systemInstruction?: string,
    modelName: string = DEFAULT_MODELS.text
  ): Promise<LLMResponse> {
    const contents = this.mapMessagesToGemini(messages);

    const systemTools: Tool[] | undefined = tools ? [{
      functionDeclarations: tools.map(t => ({
        name: t.function.name,
        description: t.function.description,
        parameters: this.mapToGeminiSchema(t.function.parameters)
      } as FunctionDeclaration))
    }] : undefined;

    const result = await this.client.models.generateContent({
      model: modelName,
      contents,
      config: {
        systemInstruction: systemInstruction,
        tools: systemTools,
      }
    });
    const candidate = result.candidates?.[0];
    const parts = candidate?.content?.parts || [];

    let contentStr: string | null = null;
    let toolCalls: LLMToolCall[] = [];

    for (const part of parts) {
      if (part.text) {
        contentStr = (contentStr || '') + part.text;
      }
    }

    // Pull tool calls from both candidates and root (some SDK versions vary)
    if (candidate?.content?.parts) {
      for (const part of candidate.content.parts) {
        if (part.functionCall) {
          toolCalls.push({
            id: Math.random().toString(36).substring(7),
            type: 'function',
            function: {
              name: part.functionCall.name,
              arguments: JSON.stringify(part.functionCall.args)
            }
          });
        }
      }
    }

    if (result.functionCalls && toolCalls.length === 0) {
      for (const call of result.functionCalls) {
        toolCalls.push({
          id: Math.random().toString(36).substring(7),
          type: 'function',
          function: {
            name: call.name,
            arguments: JSON.stringify(call.args)
          }
        });
      }
    }

    const usage = result.usageMetadata ? {
      promptTokens: result.usageMetadata.promptTokenCount || 0,
      completionTokens: (result.usageMetadata.candidatesTokenCount || 0) + (result.usageMetadata.thoughtsTokenCount || 0),
      totalTokens: result.usageMetadata.totalTokenCount || 0
    } : undefined;

    return {
      content: contentStr,
      tool_calls: toolCalls.length > 0 ? toolCalls : undefined,
      usage,
      finishReason: candidate?.finishReason as string,
      raw: result, // Return the original SDK result for technical logging
      request: {
        contents,
        systemInstruction,
        tools: systemTools
      }
    };
  }

  async generateImage(
    prompt: string,
    modelName: string = DEFAULT_MODELS.image,
    onProgress?: (msg: string) => void,
    options: { aspectRatio?: string; imageSize?: string } = {},
    images?: string[]
  ): Promise<{ data: string; usage?: GeminiMediaUsage }> {
    if (onProgress) onProgress("Generating image...");

    const config: GenerateContentConfig = {
      responseModalities: ["IMAGE", "TEXT"],
      imageConfig: {
        aspectRatio: options.aspectRatio || '16:9',
        imageSize: options.imageSize || '1K', // Default 1K, options: '512', '1K', '2K', '4K'
      },
    };

    const requestParts: Part[] = [{ text: prompt }];

    if (images && images.length > 0) {
      for (const img of images) {
        const base64Match = img.match(/^data:(image\/[a-z]+);base64,(.+)$/);
        if (base64Match) {
          requestParts.push({
            inlineData: {
              mimeType: base64Match[1],
              data: base64Match[2],
            },
          });
        }
      }
    }

    const contents: Content[] = [{ parts: requestParts }];

    const result = await this.client.models.generateContent({
      model: modelName,
      contents,
      config,
    });

    const candidate = result.candidates?.[0];
    const parts = candidate?.content?.parts || [];
    let base64Data: string | undefined;

    for (const part of parts) {
      if (part.inlineData) {
        base64Data = part.inlineData.data;
      }
    }

    const imageTokens = calculateTokensForCost(modelName, 1);

    return {
      data: base64Data || '',
      usage: {
        promptTokens: result.usageMetadata?.promptTokenCount || 0,
        completionTokens: (result.usageMetadata?.candidatesTokenCount || 0) + imageTokens,
        totalTokens: (result.usageMetadata?.totalTokenCount || 0) + imageTokens,
        count: 1
      }
    };
  }

  async generateAudio(
    prompt: string,
    modelName: string = DEFAULT_MODELS.music,
    onProgress?: (msg: string) => void
  ): Promise<{ data: string; usage?: GeminiMediaUsage }> {
    if (onProgress) onProgress("Generating audio...");
    const result = await this.client.models.generateContent({
      model: modelName,
      contents: prompt,
      config: {
        responseModalities: ["AUDIO", "TEXT"],
      }
    });

    const candidate = result.candidates?.[0];
    const parts = candidate?.content?.parts || [];
    let base64Data: string | undefined;

    for (const part of parts) {
      if (part.inlineData) {
        base64Data = part.inlineData.data;
      }
    }

    const audioTokens = calculateTokensForCost(modelName, 1);

    return {
      data: base64Data || '',
      usage: {
        promptTokens: result.usageMetadata?.promptTokenCount || 0,
        completionTokens: (result.usageMetadata?.candidatesTokenCount || 0) + audioTokens,
        totalTokens: (result.usageMetadata?.totalTokenCount || 0) + audioTokens,
        count: 1
      }
    };
  }

  async generateVideo(
    prompt: string,
    modelName: string = DEFAULT_MODELS.video,
    onProgress?: (msg: string) => void,
    options: {
      resolution?: '720p' | '1080p' | '4k';
      aspectRatio?: '16:9' | '9:16';
      durationSeconds?: 4 | 6 | 8;
    } = {},
    images?: string[]
  ): Promise<{ videoUrl: string; usage?: GeminiMediaUsage }> {
    if (modelName.includes('lite')) {
      return this.createVideoLite(prompt, modelName, onProgress, options, images);
    } else {
      return this.createVideo(prompt, modelName, onProgress, options, images);
    }
  }

  private async createVideo(
    prompt: string,
    modelName: string,
    onProgress?: (msg: string) => void,
    options: VideoGenerationOptions = {},
    images?: string[]
  ): Promise<{ videoUrl: string; usage?: GeminiMediaUsage }> {
    const videoConfig: VideoConfigPayload = {
      resolution: options.resolution || '720p',
      aspectRatio: options.aspectRatio || '16:9',
      durationSeconds: options.durationSeconds || 4,
      sampleCount: 1,
    };

    const generateVideoPayload: Record<string, unknown> = {
      model: modelName,
      config: videoConfig,
    };

    if (prompt) {
      generateVideoPayload.prompt = prompt;
    }

    if (images && images.length > 0) {
      const referenceImagesPayload: Array<Record<string, unknown>> = [];
      for (const img of images) {
        const m = img.match(/^data:(image\/[a-z]+);base64,(.+)$/);
        if (m) {
          referenceImagesPayload.push({
            image: {
              imageBytes: m[2],
              mimeType: m[1]
            },
            referenceType: 'asset' // Using lowercase string as currently mapped in other parts
          });
        }
      }
      
      if (referenceImagesPayload.length > 0) {
        videoConfig.referenceImages = referenceImagesPayload;
        // MUST be 8 when using reference images
        videoConfig.durationSeconds = 8;
      }
    }

    // Also must be 8 for 1080p or 4k
    if (videoConfig.resolution === '1080p' || videoConfig.resolution === '4k') {
      videoConfig.durationSeconds = 8;
    }

    const models = this.client.models as GenaiModelsWithVideo;
    let operation = await models.generateVideos(generateVideoPayload);

    return this.pollVideoOperation(operation, modelName, onProgress);
  }

  private async createVideoLite(
    prompt: string,
    modelName: string,
    onProgress?: (msg: string) => void,
    options: VideoGenerationOptions = {},
    images?: string[]
  ): Promise<{ videoUrl: string; usage?: GeminiMediaUsage }> {
    const videoConfig: Record<string, unknown> = {
      resolution: options.resolution || '720p',
      aspectRatio: options.aspectRatio || '16:9',
      durationSeconds: options.durationSeconds || 4,
      sampleCount: 1,
    };

    const request: Record<string, unknown> = {
      model: modelName,
      prompt: prompt,
      config: videoConfig,
    };

    if (images && images.length > 0) {
      // Lite models support 1 primary image for animation (Image object)
      const m = images[0].match(/^data:(image\/[a-z]+);base64,(.+)$/);
      if (m) {
        request.image = {
          imageBytes: m[2],
          mimeType: m[1]
        };
      }
    }

    const models = this.client.models as GenaiModelsWithVideo;
    let operation = await models.generateVideos(request);

    return this.pollVideoOperation(operation, modelName, onProgress);
  }

  private async pollVideoOperation(
    operation: GenerateVideosOperation,
    modelName: string,
    onProgress?: (msg: string) => void
  ): Promise<{ videoUrl: string; usage?: GeminiMediaUsage }> {
    const client = this.client as GoogleGenAI & GenaiClientWithVideoOps;
    while (!operation.done) {
      if (onProgress) onProgress("Generating video (this may take a minute)...");
      await new Promise((resolve) => setTimeout(resolve, 10000));
      operation = await client.operations.getVideosOperation({
        operation,
      });
    }

    const videoData = operation.response?.generatedVideos?.[0];
    let videoUri = videoData?.video?.uri ?? '';

    if (videoUri && videoUri.includes('generativelanguage.googleapis.com')) {
      const separator = videoUri.includes('?') ? '&' : '?';
      videoUri += `${separator}key=${this.apiKey}`;
    }

    const videoDuration = videoData?.durationSeconds || 4;
    const videoTokens = calculateTokensForCost(modelName, videoDuration);

    return {
      videoUrl: videoUri,
      usage: {
        promptTokens: 0,
        completionTokens: videoTokens,
        totalTokens: videoTokens,
        duration: videoDuration
      }
    };
  }

  private mapMessagesToGemini(messages: LLMMessage[]): Content[] {
    return messages
      .filter((m) => m.role !== 'system')
      .map((m) => {
        const role = m.role === 'assistant' ? 'model' : 'user';
        const parts: Part[] = [];

        if (m.content) {
          parts.push({ text: m.content });
        }

        if (m.tool_calls) {
          for (const tc of m.tool_calls) {
            parts.push({
              functionCall: {
                name: tc.function.name,
                args: JSON.parse(tc.function.arguments) as Record<string, unknown>,
              },
            });
          }
        }

        if (m.role === 'tool' && m.name) {
          parts.push({
            functionResponse: {
              name: m.name,
              response: JSON.parse(m.content) as Record<string, unknown>,
            },
          });
        }

        if (m.images) {
          for (const img of m.images) {
            const base64Match = img.match(/^data:(image\/[a-z]+);base64,(.+)$/);
            if (base64Match) {
              parts.push({
                inlineData: {
                  mimeType: base64Match[1],
                  data: base64Match[2],
                },
              });
            } else {
              parts.push({
                inlineData: {
                  mimeType: 'image/jpeg',
                  data: img,
                },
              });
            }
          }
        }

        return { role, parts };
      });
  }

  private mapToGeminiSchema(schema: unknown): Schema | undefined {
    if (schema == null || typeof schema !== 'object') return undefined;

    const raw = schema as Record<string, unknown>;
    const typeStr = String(raw.type ?? 'string').toUpperCase();
    const mappedType = Type[typeStr as keyof typeof Type] || Type.STRING;

    const result: Record<string, unknown> = {
      type: mappedType,
      description: raw.description,
      nullable: raw.nullable,
      minItems: raw.minItems,
      maxItems: raw.maxItems,
      minimum: raw.minimum,
      maximum: raw.maximum,
      minLength: raw.minLength,
      maxLength: raw.maxLength,
    };

    if (raw.properties && typeof raw.properties === 'object') {
      const props = raw.properties as Record<string, unknown>;
      const mappedProps: Record<string, Schema> = {};
      for (const key of Object.keys(props)) {
        const nested = this.mapToGeminiSchema(props[key]);
        if (nested) mappedProps[key] = nested;
      }
      result.properties = mappedProps;
    }

    if (Array.isArray(raw.required)) {
      result.required = raw.required as string[];
    }

    if (raw.items !== undefined) {
      const items = this.mapToGeminiSchema(raw.items);
      if (items) result.items = items;
    }

    if (Array.isArray(raw.enum)) {
      result.enum = raw.enum as string[];
    }

    return result as Schema;
  }
}
