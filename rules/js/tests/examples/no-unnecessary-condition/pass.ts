export function size(xs: string[] | undefined): number {
  if (xs) {
    return xs.length;
  }
  return 0;
}
