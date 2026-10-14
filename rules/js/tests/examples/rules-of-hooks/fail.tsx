import { useState } from "react";

export function Counter({ on }: { on: boolean }) {
  if (!on) return null;
  const [n] = useState(0);
  return <p>{n}</p>;
}
