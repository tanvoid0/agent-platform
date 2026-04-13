import React, { useCallback, useMemo, useState } from 'react';
import { Button } from '@/components/ui/button';
import { Label } from '@/components/ui/label';
import { Textarea } from '@/components/ui/textarea';
import { Input } from '@/components/ui/input';
import type {
  PlanningFormAnswers,
  PlanningFormField,
  PlanningFormSpec,
} from '../core/llm/types';

type DraftValue = string | boolean | string[] | '__unset__';

function initialDraft(fields: PlanningFormField[]): Record<string, DraftValue> {
  const d: Record<string, DraftValue> = {};
  for (const f of fields) {
    if (f.kind === 'boolean') d[f.id] = f.required ? '__unset__' : false;
    else if (f.kind === 'multi_select') d[f.id] = [];
    else d[f.id] = '';
  }
  return d;
}

function formatAnswerForDisplay(field: PlanningFormField, v: unknown): string {
  if (field.kind === 'multi_select' && Array.isArray(v)) return v.length ? v.join(', ') : '—';
  if (field.kind === 'boolean') {
    if (v === true) return 'Yes';
    if (v === false) return 'No';
    return '—';
  }
  if (typeof v === 'string' && v.trim()) return v.trim();
  return '—';
}

export interface PlanningFormBlockProps {
  spec: PlanningFormSpec;
  status: 'open' | 'submitted' | undefined;
  savedAnswers?: PlanningFormAnswers;
  historyIndex: number;
  disabled?: boolean;
  onSubmit: (historyIndex: number, spec: PlanningFormSpec, answers: PlanningFormAnswers) => void | Promise<void>;
}

export const PlanningFormBlock: React.FC<PlanningFormBlockProps> = ({
  spec,
  status,
  savedAnswers,
  historyIndex,
  disabled,
  onSubmit,
}) => {
  const submitted = status === 'submitted';
  const [draft, setDraft] = useState<Record<string, DraftValue>>(() => initialDraft(spec.fields));
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const setField = useCallback((id: string, v: DraftValue) => {
    setDraft((prev) => ({ ...prev, [id]: v }));
    setError(null);
  }, []);

  const toggleMulti = useCallback((id: string, option: string) => {
    setDraft((prev) => {
      const cur = prev[id];
      const arr = Array.isArray(cur) ? [...cur] : [];
      const i = arr.indexOf(option);
      if (i >= 0) arr.splice(i, 1);
      else arr.push(option);
      return { ...prev, [id]: arr };
    });
    setError(null);
  }, []);

  const validate = useCallback((): PlanningFormAnswers | null => {
    const out: PlanningFormAnswers = {};
    for (const f of spec.fields) {
      const raw = draft[f.id];
      if (f.kind === 'boolean') {
        if (f.required && raw === '__unset__') {
          setError(`Please answer: ${f.label}`);
          return null;
        }
        out[f.id] = raw === '__unset__' ? false : Boolean(raw);
        continue;
      }
      if (f.kind === 'multi_select') {
        const arr = Array.isArray(raw) ? raw : [];
        if (f.required && arr.length === 0) {
          setError(`Select at least one for: ${f.label}`);
          return null;
        }
        out[f.id] = arr;
        continue;
      }
      if (f.kind === 'single_select') {
        const s = typeof raw === 'string' ? raw.trim() : '';
        if (f.required && !s) {
          setError(`Please choose an option for: ${f.label}`);
          return null;
        }
        out[f.id] = s;
        continue;
      }
      const s = typeof raw === 'string' ? raw.trim() : '';
      if (f.required && !s) {
        setError(`Please fill in: ${f.label}`);
        return null;
      }
      out[f.id] = s;
    }
    return out;
  }, [draft, spec.fields]);

  const handleSubmit = useCallback(async () => {
    if (submitted || disabled || submitting) return;
    const answers = validate();
    if (!answers) return;
    setSubmitting(true);
    setError(null);
    try {
      await onSubmit(historyIndex, spec, answers);
    } finally {
      setSubmitting(false);
    }
  }, [disabled, historyIndex, onSubmit, spec, submitted, submitting, validate]);

  const readOnlyAnswers = useMemo(() => savedAnswers ?? {}, [savedAnswers]);

  if (submitted) {
    return (
      <div className="mt-4 rounded-2xl border border-teal-100 bg-teal-50/60 p-4 space-y-2">
        <p className="text-[10px] font-black uppercase tracking-widest text-teal-900">Planning — submitted</p>
        {savedAnswers && Object.keys(savedAnswers).length > 0 ? (
          <ul className="text-[11px] text-zinc-700 space-y-1.5">
            {spec.fields.map((f) => (
              <li key={f.id}>
                <span className="font-semibold text-zinc-800">{f.label}: </span>
                <span>{formatAnswerForDisplay(f, readOnlyAnswers[f.id])}</span>
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-[11px] text-zinc-600">Answers were sent in the chat thread.</p>
        )}
      </div>
    );
  }

  return (
    <div className="mt-4 rounded-2xl border border-teal-200/80 bg-white/90 p-4 space-y-4 shadow-sm">
      {spec.title && (
        <p className="text-[11px] font-black uppercase tracking-widest text-teal-900">{spec.title}</p>
      )}
      {spec.description && (
        <p className="text-[12px] text-zinc-600 leading-snug">{spec.description}</p>
      )}

      <div className="space-y-4">
        {spec.fields.map((field) => (
          <div key={field.id} className="space-y-2">
            <Label className="text-[11px] font-bold text-zinc-800">
              {field.label}
              {field.required && <span className="text-red-500 ml-0.5">*</span>}
            </Label>
            {field.helpText && <p className="text-[10px] text-zinc-500 leading-snug">{field.helpText}</p>}

            {field.kind === 'boolean' && (
              <div className="flex flex-wrap gap-2">
                <Button
                  type="button"
                  variant={draft[field.id] === true ? 'default' : 'outline'}
                  size="sm"
                  disabled={Boolean(disabled) || submitting}
                  className="rounded-lg text-[10px] font-black uppercase tracking-wider"
                  onClick={() => setField(field.id, true)}
                >
                  Yes
                </Button>
                <Button
                  type="button"
                  variant={draft[field.id] === false ? 'default' : 'outline'}
                  size="sm"
                  disabled={Boolean(disabled) || submitting}
                  className="rounded-lg text-[10px] font-black uppercase tracking-wider"
                  onClick={() => setField(field.id, false)}
                >
                  No
                </Button>
              </div>
            )}

            {field.kind === 'single_select' && field.options && (
              <div className="flex flex-wrap gap-2">
                {field.options.map((opt) => (
                  <Button
                    key={opt}
                    type="button"
                    variant={draft[field.id] === opt ? 'default' : 'outline'}
                    size="sm"
                    disabled={Boolean(disabled) || submitting}
                    className="rounded-lg text-[10px] font-black uppercase tracking-wider max-w-full whitespace-normal h-auto py-2 text-left"
                    onClick={() => setField(field.id, opt)}
                  >
                    {opt}
                  </Button>
                ))}
              </div>
            )}

            {field.kind === 'multi_select' && field.options && (
              <div className="flex flex-wrap gap-2">
                {field.options.map((opt) => {
                  const cur = draft[field.id];
                  const selected = Array.isArray(cur) && cur.includes(opt);
                  return (
                    <Button
                      key={opt}
                      type="button"
                      variant={selected ? 'default' : 'outline'}
                      size="sm"
                      disabled={Boolean(disabled) || submitting}
                      className="rounded-lg text-[10px] font-black uppercase tracking-wider max-w-full whitespace-normal h-auto py-2 text-left"
                      onClick={() => toggleMulti(field.id, opt)}
                    >
                      {opt}
                    </Button>
                  );
                })}
              </div>
            )}

            {field.kind === 'text' && (
              <Input
                value={(() => {
                  const v = draft[field.id];
                  return typeof v === 'string' ? v : '';
                })()}
                onChange={(e) => setField(field.id, e.target.value)}
                disabled={Boolean(disabled) || submitting}
                className="text-[12px] rounded-xl border-zinc-200"
                placeholder="…"
              />
            )}

            {field.kind === 'textarea' && (
              <Textarea
                value={(() => {
                  const v = draft[field.id];
                  return typeof v === 'string' ? v : '';
                })()}
                onChange={(e) => setField(field.id, e.target.value)}
                disabled={Boolean(disabled) || submitting}
                className="text-[12px] rounded-xl border-zinc-200 min-h-[72px]"
                placeholder="…"
              />
            )}
          </div>
        ))}
      </div>

      {error && <p className="text-[11px] text-red-600 font-medium">{error}</p>}

      <Button
        type="button"
        disabled={Boolean(disabled) || submitting}
        onClick={() => void handleSubmit()}
        className="w-full rounded-xl bg-teal-700 text-white hover:bg-teal-800 text-[10px] font-black uppercase tracking-widest"
      >
        {submitting ? 'Sending…' : 'Submit answers'}
      </Button>
    </div>
  );
};
