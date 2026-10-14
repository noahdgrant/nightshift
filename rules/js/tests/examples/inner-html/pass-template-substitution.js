export function show(el, html, key) {
  el[`${key}`] = html;
  el[`inner`] = html;
}
export function mix(el, html, key) {
  el[`innerHTML${key}`] = html;
}
