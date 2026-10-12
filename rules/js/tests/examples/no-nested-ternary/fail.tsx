export function Badge({ n }: { n: number }) {
  return <span>{n > 0 ? "up" : n < 0 ? "down" : "flat"}</span>;
}
