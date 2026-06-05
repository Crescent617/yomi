import { LocalFSProvider } from "./localProvider";
import type { FileSystemProvider } from "./provider";

export function createFSProvider(): FileSystemProvider {
  // For now, always use local provider.
  // Future: detect remote kernel and return KernelFSProvider.
  return new LocalFSProvider();
}

export const fsProvider = createFSProvider();
