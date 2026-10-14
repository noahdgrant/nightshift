export function later(): void {
  const code: string = String(Date.now());
  setTimeout(code, 10);
}
