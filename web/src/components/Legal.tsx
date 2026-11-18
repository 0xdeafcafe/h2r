// The Game Content Usage Rules notice (src/legal.ts), as a paragraph with
// the rules linked. Each side passes its own name and class.

import { COPYRIGHT, FAN_PROJECT, RULES_URL, noticeParts } from "../legal";

export default function Legal(props: { name: string; class?: string }) {
  const p = () => noticeParts(props.name);
  return (
    <p class={props.class ?? "legal"} data-legal>
      {p().before}<a href={RULES_URL} target="_blank" rel="noopener noreferrer">{p().link}</a>{p().after} {COPYRIGHT} {FAN_PROJECT}
    </p>
  );
}
