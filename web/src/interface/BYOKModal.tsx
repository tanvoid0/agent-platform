import { X } from 'lucide-react';
import React from 'react';
import { Button } from '@/components/ui/button';
import { ModalBackdrop, ModalRoot } from './components/ModalChrome';
import { AiClientsSettingsPanel } from './settings/AiClientsSettingsPanel';

interface BYOKModalProps {
  onClose: () => void;
}

const BYOKModal: React.FC<BYOKModalProps> = ({ onClose }) => {
  return (
    <ModalRoot paddingClassName="p-6" className="pointer-events-auto overflow-hidden">
      <ModalBackdrop tone="lightXl" onRequestClose={onClose} />
      <div className="relative w-full max-w-md bg-white rounded-[40px] shadow-[0_32px_64px_-12px_rgba(0,0,0,0.1)] p-8 md:p-10 border border-zinc-100 max-h-[90vh] overflow-y-auto">
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          onClick={onClose}
          className="absolute top-6 right-6 z-10 text-zinc-300 hover:text-zinc-600"
          aria-label="Close"
        >
          <X size={18} />
        </Button>

        <AiClientsSettingsPanel variant="modal" onSaved={onClose} />
      </div>
    </ModalRoot>
  );
};

export default BYOKModal;
