import { Component, type ErrorInfo, type ReactNode } from "react";

type Props = { children: ReactNode; fallback?: ReactNode };

type State = { error: Error | null };

/**
 * Isolates pixel/canvas failures from the rest of the process workspace (presentation-only).
 */
export class PixelErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    console.warn("[pixel]", error.message, info.componentStack);
  }

  render(): ReactNode {
    if (this.state.error) {
      return this.props.fallback ?? null;
    }
    return this.props.children;
  }
}
