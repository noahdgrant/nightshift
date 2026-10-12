// The JSX types, so tsc checks the .tsx examples without installing React's types.
declare namespace JSX {
  type Element = Record<string, unknown>;
  type IntrinsicElements = Record<string, Record<string, unknown>>;
}
