export function distinct<T extends { id: string }>(items: T[]) {
  return [...new Map(items.map((i) => [i.id, i])).values()];
}
