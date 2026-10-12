const ready = Promise.resolve(true);

export function check(): number {
  if (ready) {
    return 1;
  }
  return 0;
}
