export class Walker {
  walk(xs) {
    function flatten(ys) {
      return ys.flat();
    }
    return flatten(xs);
  }
}
