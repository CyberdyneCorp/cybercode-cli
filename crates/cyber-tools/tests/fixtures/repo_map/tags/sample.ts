export interface Store {
  get(key: string): string | undefined;
}

export type Key = string;

export enum Mode {
  Read,
  Write,
}

export class MemoryStore implements Store {
  get(key: string): string | undefined {
    return undefined;
  }
}

export function open(path: string): Store {
  return new MemoryStore();
}

namespace Internal {
  export const value = 1;
}
