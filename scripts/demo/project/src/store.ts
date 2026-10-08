import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { slug } from "./slug";

export interface Link {
  url: string;
  slug: string;
  createdAt: string;
  clicks: number;
}

/** The links, kept in memory and written to a JSON file after each change. */
export class Store {
  private links = new Map<string, Link>();

  constructor(private file: string) {
    if (existsSync(file)) {
      for (const link of JSON.parse(readFileSync(file, "utf8")) as Link[]) {
        this.links.set(link.slug, link);
      }
    }
  }

  add(url: string): Link {
    let id = slug();
    while (this.links.has(id)) id = slug();
    const link: Link = { url, slug: id, createdAt: new Date().toISOString(), clicks: 0 };
    this.links.set(id, link);
    this.save();
    return link;
  }

  get(id: string): Link | undefined {
    return this.links.get(id);
  }

  /** The link behind `id`, its click counted. */
  click(id: string): Link | undefined {
    const link = this.links.get(id);
    if (link) {
      link.clicks++;
      this.save();
    }
    return link;
  }

  all(): Link[] {
    return [...this.links.values()];
  }

  private save() {
    mkdirSync(dirname(this.file), { recursive: true });
    writeFileSync(this.file, JSON.stringify(this.all(), null, 2));
  }
}
