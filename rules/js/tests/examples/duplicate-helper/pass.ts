export class Tree {
  flatten(): number[] {
    return [1];
  }
}

export function flattenTree(tree: Tree): number[] {
  return tree.flatten();
}
