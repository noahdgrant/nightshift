export function load(read) {
  try {
    return read();
  } catch {
    return null;
  }
}
