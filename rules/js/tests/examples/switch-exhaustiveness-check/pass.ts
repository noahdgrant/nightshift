export type Shape = "circle" | "square";

export function sides(shape: Shape): number {
  switch (shape) {
    case "circle":
      return 0;
    case "square":
      return 4;
  }
}
