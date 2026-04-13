import { QueryClient } from "@tanstack/react-query";

export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 0,
      retry: (failureCount, error) => {
        if (failureCount >= 2) return false;
        return true;
      },
    },
    mutations: {
      retry: 0,
    },
  },
});
