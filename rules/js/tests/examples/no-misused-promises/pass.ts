const ready = Promise.resolve(true);

export async function check(): Promise<number> {
  if (await ready) {
    return 1;
  }
  return 0;
}
