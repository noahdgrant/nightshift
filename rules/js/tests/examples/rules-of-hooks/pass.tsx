import { useState } from "react";

export function Counter({ on }: { on: boolean }) {
  const [n] = useState(0);
  return on ? <p>{n}</p> : null;
}
