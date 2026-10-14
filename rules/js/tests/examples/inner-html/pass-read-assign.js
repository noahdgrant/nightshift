export function read(el) {
  let x = el.innerHTML;
  x = el.innerHTML;
  let out = "";
  out += el.outerHTML;
  return [x, out];
}
