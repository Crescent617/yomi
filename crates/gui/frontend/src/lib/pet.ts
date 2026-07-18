export const PET_COMPACT_SIZE = { width: 152, height: 112 } as const;
export const PET_EXPANDED_SIZE = { width: 200, height: 216 } as const;

export function getPetWindowSize(bubbleVisible: boolean) {
  return bubbleVisible ? PET_EXPANDED_SIZE : PET_COMPACT_SIZE;
}
