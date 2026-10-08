# shorty

A tiny link shortener: paste a long URL, get a short one back. One Bun process, links kept in a JSON file.

```sh
bun install
bun run dev        # http://localhost:3000
bun test
```

## API

| Method | Path               | What it does                                  |
|--------|--------------------|-----------------------------------------------|
| POST   | `/api/links`       | `{ "url": "https://…" }` → `{ "slug": "k3x9" }` |
| GET    | `/api/links`       | every link                                     |
| GET    | `/api/links/:slug` | the link and its click count                   |
| GET    | `/:slug`           | redirects to the link, counts the click        |

## Layout

- `src/server.ts`: routes
- `src/store.ts`: links on disk (`data/links.json`)
- `src/slug.ts`: short ids
- `public/index.html`: the page
- `test/`: `bun test`
