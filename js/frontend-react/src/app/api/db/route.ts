import { connect } from "./database";

export const GET = () => {
  return new Response(null, { status: 403 });
};

// Define the POST request handler function
export const POST = async (req: Request) => {
  try {
    const reqJson = await req.json();
    const { db: path, query, bindings, mode } = reqJson;
    const db = await connect(path);

    if (!query) {
      return new Response("No query specified", {
        status: 400,
      });
    }

    switch (mode) {
      case "exec": {
        await db.exec(query.toString());
        return new Response("null", {
          headers: { "Content-Type": "application/json" },
          status: 200,
        });
      }

      case "run": {
        await db.run(query.toString(), ...(bindings ?? []));

        return new Response("null", {
          headers: { "Content-Type": "application/json" },
          status: 200,
        });
      }

      case "many": {
        for (const b of bindings ?? []) {
          await db.run(query.toString(), ...(b ?? []));
        }

        return new Response("null", {
          headers: { "Content-Type": "application/json" },
          status: 200,
        });
      }

      case "get": {
        const result = await db.get(query.toString(), ...(bindings ?? []));
        // Return the items as a JSON response with status 200
        return new Response(JSON.stringify([result]), {
          headers: { "Content-Type": "application/json" },
          status: 200,
        });
      }

      default:
      case "query": {
        const result = await db.all(query.toString(), ...(bindings ?? []));
        // Return the items as a JSON response with status 200
        return new Response(JSON.stringify(result), {
          headers: { "Content-Type": "application/json" },
          status: 200,
        });
      }
    }
  } catch (e) {
    return new Response(`${e}`, { status: 500 });
  }
};
