import React from 'react'
import ReactMarkdown from 'react-markdown'

export const UserBriefSection: React.FC<{
  userBrief: string
  referenceImages: string[]
  outputType: string
}> = ({ userBrief, referenceImages, outputType }) => (
  <div className="mb-8">
    <div className="flex items-center gap-2 mb-4">
      <p className="text-[10px] font-black uppercase tracking-widest text-zinc-400">User Brief</p>
      <div className="h-px flex-1 bg-zinc-100" />
    </div>
    {userBrief ? (
      <div className="space-y-4">
        <div className="markdown-content text-xs text-zinc-600 leading-relaxed font-medium bg-white/40 p-4 rounded-xl border border-zinc-100/50 max-h-[300px] overflow-y-auto custom-scrollbar">
          <ReactMarkdown>{userBrief}</ReactMarkdown>
        </div>

        {(outputType === 'image' || outputType === 'video') && referenceImages.length > 0 && (
          <div className="flex flex-col gap-2">
            <p className="text-[9px] font-black uppercase tracking-widest text-zinc-400">
              Brief Logic References
            </p>
            <div className="grid grid-cols-3 gap-2">
              {referenceImages.map((img, idx) => (
                <div
                  key={idx}
                  className="aspect-square rounded-xl overflow-hidden border border-zinc-100 shadow-sm bg-zinc-50"
                >
                  <img src={img} alt={`Ref ${idx}`} className="w-full h-full object-cover" />
                </div>
              ))}
            </div>
          </div>
        )}
      </div>
    ) : (
      <p className="text-xs text-zinc-400 italic">
        No active brief. Talk to the Lead Agent to define your project.
      </p>
    )}
  </div>
)
