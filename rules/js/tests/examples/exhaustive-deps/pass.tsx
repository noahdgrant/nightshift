import { useEffect } from "react";

export function Title({ title }: { title: string }) {
  useEffect(() => {
    document.title = title;
  }, [title]);
  return <h1>{title}</h1>;
}
