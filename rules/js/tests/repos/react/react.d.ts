// The hooks the examples call, so tsc checks them without installing React's types.
declare module "react" {
  export function useState<T>(initial: T): [T, (next: T) => void];
  export function useEffect(effect: () => void, deps?: readonly unknown[]): void;
}
