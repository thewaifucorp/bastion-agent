/**
 * Wrap a cursor-paginated `/v1/*` list endpoint in an `AsyncIterable`, so
 * callers can `for await (const task of paginate(...))` instead of hand-
 * rolling the `next_cursor` loop. Purely a convenience over `fetchPage` —
 * see the "SDK conveniences" list in the planning doc
 * (`US-external-control-plane-sdk.md`: "pagination iterator").
 */
export async function* paginate<TItem>(
  fetchPage: (cursor: string | undefined) => Promise<{ items: TItem[]; next_cursor: string | null }>,
): AsyncGenerator<TItem, void, undefined> {
  let cursor: string | undefined;
  for (;;) {
    const page = await fetchPage(cursor);
    for (const item of page.items) {
      yield item;
    }
    if (page.next_cursor === null) return;
    cursor = page.next_cursor;
  }
}
