const idIndexes = new WeakMap<ReadonlyArray<{ id: string }>, Set<string>>();

/**
 * Appends a cursor page without changing server order. The ID index follows the
 * returned immutable array, avoiding a full re-index of all previously loaded
 * pages on every Load More while also rejecting duplicates inside one page.
 */
export function appendUnique<T extends { id: string }>(current: T[], incoming: T[]): T[] {
  let known = idIndexes.get(current);
  if (!known) known = new Set(current.map((item) => item.id));

  const additions: T[] = [];
  for (const item of incoming) {
    if (known.has(item.id)) continue;
    known.add(item.id);
    additions.push(item);
  }
  if (additions.length === 0) return current;

  const result = current.concat(additions);
  // The old immutable array is no longer the owner of this now-extended index.
  // If a caller branches from it later, it will safely rebuild its own index.
  idIndexes.delete(current);
  idIndexes.set(result, known);
  return result;
}

export function tableWindow(length: number, requestedPage: number, windowSize: number, printing = false) {
  const pages = Math.max(1, Math.ceil(length / windowSize));
  const page = Math.max(0, Math.min(requestedPage, pages - 1));
  return printing
    ? { page, pages, start: 0, end: length }
    : { page, pages, start: page * windowSize, end: Math.min(length, (page + 1) * windowSize) };
}
