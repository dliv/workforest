import type { VersionResponse } from "./generated/VersionResponse";

interface Env {
  DB: D1Database;
  LATEST_VERSION: string;
}

export default {
  async fetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    const url = new URL(request.url);

    if (url.pathname !== "/api/latest") {
      return new Response("Not found", { status: 404 });
    }

    if (request.method !== "GET") {
      return new Response("Method not allowed", { status: 405 });
    }

    const version = url.searchParams.get("v") || "unknown";
    const cf = (request as any).cf || {};
    const city = cf.city || null;
    const country = cf.country || null;
    const timestamp = new Date().toISOString();

    ctx.waitUntil(
      env.DB.prepare(
        "INSERT INTO events (city, country, version, timestamp) VALUES (?, ?, ?, ?)",
      )
        .bind(city, country, version, timestamp)
        .run()
        .catch((e) => console.error("D1 write failed:", e))
    );

    const response: VersionResponse = {
      version: env.LATEST_VERSION,
    };
    return Response.json(response);
  },
};
