import { bodyColour, vitalsNumber, type DamageMode, type Vitals } from "../map/vitals";

function track(kind: string) {
  const root = document.createElement("div");
  root.className = `v3-vitals-${kind}`;
  const fill = document.createElement("i");
  const chip = document.createElement("i");
  root.append(fill, chip);
  return { root, fill, chip };
}

type Track = ReturnType<typeof track>;

function draw(bar: Track, value: number, edge: number | null) {
  const filled = Math.min(1, Math.max(0, value)) * 100;
  const chipped = edge === null ? 0 : Math.min(1, Math.max(0, edge)) * 100 - filled;
  bar.fill.style.width = `${filled}%`;
  bar.chip.style.left = `${filled}%`;
  bar.chip.style.width = `${Math.max(0, chipped)}%`;
}

export class TagDamage {
  private over = track("over");
  private shield = track("shield");
  private body = track("body");
  private bars = document.createElement("div");
  private number = document.createElement("span");
  private layout = "";

  constructor(tag: HTMLElement, private plate: HTMLElement) {
    this.bars.className = "v3-vitals";
    this.bars.setAttribute("aria-hidden", "true");
    this.bars.append(this.over.root, this.shield.root, this.body.root);
    this.number.className = "v3-vitals-number";
    plate.append(this.number);
    // The pip is absolutely positioned and does not affect the tag's size.
    tag.append(this.bars);
  }

  /** Updates the bars and reports when the measured tag may change size. */
  update(mode: DamageMode, vitals: Vitals | undefined): boolean {
    const v = mode === "off" ? undefined : vitals;
    this.bars.hidden = !v;
    this.plate.classList.toggle("v3-vitals-flare", !!v?.flare);
    const number = mode === "numbers" ? vitalsNumber(v) : "";
    if (this.number.textContent !== number) this.number.textContent = number;
    if (v) {
      this.over.root.hidden = v.shield <= 1 && (v.shieldChip ?? 0) <= 1;
      this.body.root.hidden = !v.showBody;
      this.shield.root.classList.toggle("v3-vitals-break", v.breaking);
      draw(this.over, v.shield - 1, v.shieldChip === null ? null : v.shieldChip - 1);
      draw(this.shield, v.shield, v.shieldChip);
      if (v.showBody) {
        this.body.fill.style.backgroundColor = bodyColour(v.body);
        draw(this.body, v.body, v.bodyChip);
      }
    }
    const layout = v ? `${!this.over.root.hidden}/${v.showBody}/${number}` : "";
    if (layout === this.layout) return false;
    this.layout = layout;
    return true;
  }
}
