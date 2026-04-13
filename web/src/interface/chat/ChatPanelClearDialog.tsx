import React from 'react'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'

export const ChatPanelClearDialog: React.FC<{
  open: boolean
  onOpenChange: (open: boolean) => void
  agentName: string
  onConfirm: () => void
}> = ({ open, onOpenChange, agentName, onConfirm }) => (
  <AlertDialog open={open} onOpenChange={onOpenChange}>
    <AlertDialogContent size="sm" overlayClassName="z-[200]" className="z-[210]">
      <AlertDialogHeader>
        <AlertDialogTitle>Clear chat?</AlertDialogTitle>
        <AlertDialogDescription>
          This removes all messages with{' '}
          <span className="font-medium text-foreground">{agentName}</span> in this project. This cannot be
          undone.
        </AlertDialogDescription>
      </AlertDialogHeader>
      <AlertDialogFooter>
        <AlertDialogCancel>Cancel</AlertDialogCancel>
        <AlertDialogAction variant="destructive" onClick={onConfirm}>
          Clear chat
        </AlertDialogAction>
      </AlertDialogFooter>
    </AlertDialogContent>
  </AlertDialog>
)
