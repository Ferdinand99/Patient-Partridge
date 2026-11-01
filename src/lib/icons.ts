// Icons come from the inline sprite in index.html (`<symbol id="i-NAME">`). They are built with
// DOM calls, never from strings.

const SVG_NS = "http://www.w3.org/2000/svg";

export function icon(name: string): SVGSVGElement {
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("class", "icon");
  svg.setAttribute("aria-hidden", "true");
  const use = document.createElementNS(SVG_NS, "use");
  use.setAttribute("href", `#i-${name}`);
  svg.append(use);
  return svg;
}
