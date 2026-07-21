import { test } from "node:test";
import assert from "node:assert/strict";
import { paginate } from "../src/pagination.js";

test("paginate collects every item across multiple pages in order", async () => {
  const pages: Record<string, { items: number[]; next_cursor: string | null }> = {
    "": { items: [1, 2], next_cursor: "c1" },
    c1: { items: [3, 4], next_cursor: "c2" },
    c2: { items: [5], next_cursor: null },
  };
  const calls: (string | undefined)[] = [];

  const fetchPage = async (cursor: string | undefined) => {
    calls.push(cursor);
    return pages[cursor ?? ""]!;
  };

  const collected: number[] = [];
  for await (const item of paginate(fetchPage)) {
    collected.push(item);
  }

  assert.deepEqual(collected, [1, 2, 3, 4, 5]);
  assert.deepEqual(calls, [undefined, "c1", "c2"]);
});

test("paginate stops after a single page when next_cursor is null immediately", async () => {
  const fetchPage = async () => ({ items: ["only"], next_cursor: null });
  const collected: string[] = [];
  for await (const item of paginate(fetchPage)) {
    collected.push(item);
  }
  assert.deepEqual(collected, ["only"]);
});

test("paginate yields nothing for an empty first page", async () => {
  const fetchPage = async () => ({ items: [] as string[], next_cursor: null });
  const collected: string[] = [];
  for await (const item of paginate(fetchPage)) {
    collected.push(item);
  }
  assert.deepEqual(collected, []);
});
