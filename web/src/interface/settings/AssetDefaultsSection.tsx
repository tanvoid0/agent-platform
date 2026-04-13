import React from 'react';
import { useCoreStore } from '../../integration/store/coreStore';
import { InfoBubble } from '../components/InfoBubble';

export const AssetDefaultsSection: React.FC = () => {
  const defaults = useCoreStore((s) => s.assetGenerationDefaults);
  const setAssetGenerationDefaults = useCoreStore((s) => s.setAssetGenerationDefaults);

  return (
    <div>
      <h2 className="text-xl font-black text-darkDelegation tracking-tight mb-1">Default asset quality</h2>
      <p className="text-sm text-zinc-500 font-medium mb-6 max-w-xl">
        Starting values when the output review modal opens (teams with manual approval before generating image or
        video). You can still change them per generation.
      </p>

      <div className="space-y-6">
        <div>
          <p className="text-[10px] font-black uppercase tracking-wider text-zinc-400 mb-3">Image</p>
          <div className="grid sm:grid-cols-2 gap-4">
            <div className="space-y-2">
              <label className="text-[10px] font-black uppercase tracking-widest text-zinc-400 flex items-center gap-1.5">
                Aspect ratio
                <InfoBubble text="Default proportions for the generated image when the review modal opens." />
              </label>
              <select
                value={defaults.image.aspectRatio}
                onChange={(e) =>
                  setAssetGenerationDefaults({ image: { aspectRatio: e.target.value } })
                }
                className="w-full bg-zinc-50 border border-zinc-200 rounded-xl px-3 py-2.5 text-xs focus:ring-2 focus:ring-darkDelegation outline-none"
              >
                <option value="1:1">1:1 Square</option>
                <option value="16:9">16:9 Cinematic</option>
                <option value="9:16">9:16 Vertical</option>
                <option value="4:3">4:3 Classic</option>
                <option value="3:2">3:2 Professional</option>
              </select>
            </div>
            <div className="space-y-2">
              <label className="text-[10px] font-black uppercase tracking-widest text-zinc-400 flex items-center gap-1.5">
                Image size
                <InfoBubble text="Higher sizes cost more time and tokens; 512px is fastest." />
              </label>
              <select
                value={defaults.image.imageSize}
                onChange={(e) =>
                  setAssetGenerationDefaults({ image: { imageSize: e.target.value } })
                }
                className="w-full bg-zinc-50 border border-zinc-200 rounded-xl px-3 py-2.5 text-xs focus:ring-2 focus:ring-darkDelegation outline-none"
              >
                <option value="512">512px (Fast)</option>
                <option value="1K">1K (Standard)</option>
                <option value="2K">2K (High res)</option>
                <option value="4K">4K (Ultra)</option>
              </select>
            </div>
          </div>
        </div>

        <div>
          <p className="text-[10px] font-black uppercase tracking-wider text-zinc-400 mb-3">Video</p>
          <div className="grid sm:grid-cols-3 gap-4">
            <div className="space-y-2">
              <label className="text-[10px] font-black uppercase tracking-widest text-zinc-400 flex items-center gap-1.5">
                Resolution
                <InfoBubble text="Higher resolution increases fidelity and processing cost." />
              </label>
              <select
                value={defaults.video.resolution}
                onChange={(e) =>
                  setAssetGenerationDefaults({ video: { resolution: e.target.value } })
                }
                className="w-full bg-zinc-50 border border-zinc-200 rounded-xl px-3 py-2.5 text-xs focus:ring-2 focus:ring-darkDelegation outline-none"
              >
                <option value="720p">720p HD</option>
                <option value="1080p">1080p Full HD</option>
                <option value="4k">4K</option>
              </select>
            </div>
            <div className="space-y-2">
              <label className="text-[10px] font-black uppercase tracking-widest text-zinc-400">Aspect ratio</label>
              <select
                value={defaults.video.aspectRatio}
                onChange={(e) =>
                  setAssetGenerationDefaults({ video: { aspectRatio: e.target.value } })
                }
                className="w-full bg-zinc-50 border border-zinc-200 rounded-xl px-3 py-2.5 text-xs focus:ring-2 focus:ring-darkDelegation outline-none"
              >
                <option value="16:9">16:9</option>
                <option value="9:16">9:16</option>
                <option value="1:1">1:1</option>
              </select>
            </div>
            <div className="space-y-2">
              <label className="text-[10px] font-black uppercase tracking-widest text-zinc-400">Duration (s)</label>
              <select
                value={defaults.video.durationSeconds}
                onChange={(e) =>
                  setAssetGenerationDefaults({
                    video: { durationSeconds: parseInt(e.target.value, 10) },
                  })
                }
                className="w-full bg-zinc-50 border border-zinc-200 rounded-xl px-3 py-2.5 text-xs focus:ring-2 focus:ring-darkDelegation outline-none"
              >
                <option value={4}>4</option>
                <option value={6}>6</option>
                <option value={8}>8</option>
              </select>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
};
