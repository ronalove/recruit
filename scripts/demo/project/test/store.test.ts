import { describe, expect, test } from "bun:test";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { isSlug } from "../src/slug";
import { Store } from "../src/store";

const file = () => join(mkdtempSync(join(tmpdir(), "shorty-")), "links.json");

describe("Store", () => {
  test("adds a link with a valid slug", () => {
    const link = new Store(file()).add("https://example.com/a/long/path");
    expect(isSlug(link.slug)).toBe(true);
    expect(link.clicks).toBe(0);
  });

  test("counts clicks", () => {
    const store = new Store(file());
    const { slug } = store.add("https://example.com");
    store.click(slug);
    store.click(slug);
    expect(store.get(slug)?.clicks).toBe(2);
  });

  test("keeps links on disk", () => {
    const path = file();
    const { slug } = new Store(path).add("https://example.com");
    expect(new Store(path).get(slug)?.url).toBe("https://example.com");
  });
});
