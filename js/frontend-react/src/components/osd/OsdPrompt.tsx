import { Heading, Subheading } from "@/components/ui-kit/heading";
import { PropertyList } from "@/components";
import { useState } from "react";
import { Divider } from "@/components/ui-kit/divider";
import { Button } from "@/components/ui-kit/button";
import { Input } from "@/components/ui-kit/input";

export function OsdPrompt({
  title,
  message,
  default: defaultValue,
  resolve,
}: {
  title?: string;
  message: string;
  default?: string;
  resolve: (v: string | undefined) => void;
}) {
  const [value, setValue] = useState<string>(defaultValue ?? "");

  return (
    <>
      <Heading>Prompt</Heading>
      <PropertyList properties={{ title, message, default: defaultValue }} />
      <Subheading>Value</Subheading>
      <Input value={value} onChange={(e) => setValue(e.target.value)} />
      <Divider className="py-2 mt-8" />
      <Button onClick={() => resolve(value)}>Submit</Button>{" "}
      <Button onClick={() => resolve(undefined)}>Cancel</Button>
    </>
  );
}
