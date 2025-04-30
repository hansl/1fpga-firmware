import * as zod from "zod";

/**
 * A version number or version string. If a string, it is expected to
 * be semver-ish.
 */
export const Version = zod
  .string()
  .min(1)
  .max(64)
  .regex(/^[0-9a-zA-Z][-0-9a-zA-Z._@()+]*$/)
  .or(zod.number())
  .describe("Version number");

export type Version = zod.TypeOf<typeof Version>;

export const UrlOrRelative = zod.url().or(
  zod
    .string()
    .min(1)
    .check(
      // Allow URL strings relative to a base, so need to validate with a random base.
      (ctx) => {
        try {
          new URL(ctx.value, "p://example.com/");
        } catch (e) {
          ctx.issues.push({
            message: `${e}`,
            input: ctx.value,
          });
        }
      },
    ),
);
