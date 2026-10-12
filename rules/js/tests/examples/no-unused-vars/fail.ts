export function sum(xs: number[]): number {
  const unused = 1;
  return xs.reduce((a, b) => a + b, 0);
}
