export function deep(a, b, c) {
  if (a) {
    if (b) {
      if (c) {
        return 3;
      }
    }
  }
  return 0;
}
