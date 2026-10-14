export function later(tick: () => void): void {
  setTimeout(tick, 10);
}
