async function save(n: number): Promise<number> {
  return await Promise.resolve(n);
}

export function saveAll(xs: number[]): void {
  xs.forEach(async (x) => {
    await save(x);
  });
}
