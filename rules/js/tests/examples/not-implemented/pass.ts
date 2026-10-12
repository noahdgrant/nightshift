export function parse(text: string): number {
  if (text === "") {
    throw new Error("parse: empty input");
  }
  return Number(text);
}
