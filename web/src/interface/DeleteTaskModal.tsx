import React from 'react';
import ConfirmModal from './components/ConfirmModal';

interface DeleteTaskModalProps {
  isOpen: boolean;
  onClose: () => void;
  onConfirm: () => void;
  taskTitle?: string;
}

const DeleteTaskModal: React.FC<DeleteTaskModalProps> = ({ isOpen, onClose, onConfirm, taskTitle }) => {
  return (
    <ConfirmModal
      isOpen={isOpen}
      onClose={onClose}
      onConfirm={() => {
        onConfirm();
        onClose();
      }}
      title="Delete Task?"
      description={
        <>
          Are you sure you want to delete{' '}
          {taskTitle ? <span className="font-semibold text-zinc-700">&quot;{taskTitle}&quot;</span> : 'this task'}? This
          action cannot be undone.
        </>
      }
      variant="danger"
      confirmLabel="Delete Task"
      footerLayout="row"
      size="sm"
      zIndexClass="z-110"
    />
  );
};

export default DeleteTaskModal;
