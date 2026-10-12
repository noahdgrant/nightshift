export function Badge({ n }: { n: number }) {
  return <span>{n > 0 ? "up" : "not up"}</span>;
}
