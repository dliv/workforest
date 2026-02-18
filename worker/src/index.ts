import type { VersionResponse } from "./generated/VersionResponse";

interface Env {
  DB: D1Database;
  KV: KVNamespace;
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
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

    // Log to D1 (best-effort, don't fail the response)
    try {
      await env.DB.prepare(
        "INSERT INTO events (city, country, version, timestamp) VALUES (?, ?, ?, ?)",
      )
        .bind(city, country, version, timestamp)
        .run();
    } catch (e) {
      console.error("D1 write failed:", e);
    }

    // Return latest version from KV
    const latest = await env.KV.get("latest_version");
    const response: VersionResponse = {
      version: latest || "0.2.3",
    };
    return Response.json(response);
  },
};
