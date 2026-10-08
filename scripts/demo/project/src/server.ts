import { isSlug } from "./slug";
import { Store } from "./store";

const store = new Store(process.env.SHORTY_DATA ?? "data/links.json");
const page = Bun.file(new URL("../public/index.html", import.meta.url));

function httpUrl(value: unknown): value is string {
  if (typeof value !== "string") return false;
  try {
    const { protocol } = new URL(value);
    return protocol === "http:" || protocol === "https:";
  } catch {
    return false;
  }
}

const server = Bun.serve({
  port: Number(process.env.PORT ?? 3000),
  async fetch(req) {
    const { pathname } = new URL(req.url);

    if (pathname === "/") return new Response(page);

    if (pathname === "/api/links") {
      if (req.method === "GET") return Response.json(store.all());
      if (req.method === "POST") {
        const body = await req.json().catch(() => ({}));
        if (!httpUrl(body.url)) return Response.json({ error: "url must be http(s)" }, { status: 400 });
        const link = store.add(body.url);
        return Response.json({ slug: link.slug }, { status: 201 });
      }
    }

    const api = pathname.match(/^\/api\/links\/([^/]+)$/);
    if (api) {
      const link = store.get(api[1]);
      return link ? Response.json(link) : Response.json({ error: "not found" }, { status: 404 });
    }

    const id = pathname.slice(1);
    const link = isSlug(id) ? store.click(id) : undefined;
    return link ? Response.redirect(link.url, 302) : new Response("Not found", { status: 404 });
  },
});

console.log(`shorty on http://localhost:${server.port}`);
