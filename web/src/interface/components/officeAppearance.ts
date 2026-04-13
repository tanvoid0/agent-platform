import { useCoreStore } from '../../integration/store/coreStore';
import type { OfficeVisualStyle } from '../../types';

/**
 * Updates scene appearance; zustand `persist` writes project-scoped `core-storage`.
 */
export function commitOfficeVisualStyle(style: OfficeVisualStyle): void {
  useCoreStore.getState().setOfficeVisualStyle(style);
}
