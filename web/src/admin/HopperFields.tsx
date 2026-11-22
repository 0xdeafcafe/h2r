import { Index } from "solid-js";
import { MAX_NAME, MAX_RECORDS, MAX_STRING, type EditHopper, type EditRecord } from "./validate";

export const TYPE_NAMES: Record<number, string> = {
  1: "Unranked free for all",
  2: "Ranked free for all",
  3: "Unranked teams",
  4: "Ranked teams",
  5: "Clan match",
};

const TYPES = Object.entries(TYPE_NAMES);
const STATUSES: Array<[number, string]> = [
  [0, "0 · Not loaded"],
  [1, "1 · OK"],
  [2, "2 · Error"],
];

function rangeLabels(type: number): [string, string, string] {
  return type >= 3 ? ["Teams", "Smallest team", "Largest team"] : ["Fewest players", "Most players", "Unused"];
}

function num(value: string): number | null {
  const n = Number(value);
  return Number.isInteger(n) && n >= 0 ? n : null;
}

export default function HopperFields(props: {
  hopper: EditHopper;
  edit: (patch: Partial<EditHopper>) => void;
  editRecord: (slot: number, patch: Partial<EditRecord>) => void;
  moveRecord: (slot: number, by: -1 | 1) => void;
  copyRecord: (slot: number) => void;
  removeRecord: (slot: number) => void;
}) {
  return (
    <details class="adm-hint adm-advanced">
      <summary>Advanced: raw fields</summary>
      <p class="adm-preset-note">
        Type and ranges control matchmaking. Use Change rules to apply a valid template. The raw name overlaps a table the game validates. Entry strings, flags and second words are part of the game variant.
      </p>
      <div class="adm-row">
        <label class="adm-field adm-grow">
          <span>Name (raw, ASCII)</span>
          <input
            value={props.hopper.name}
            maxLength={MAX_NAME}
            onInput={(e) => props.edit({ name: e.currentTarget.value })}
          />
        </label>
        <label class="adm-field">
          <span>Type</span>
          <select
            value={String(props.hopper.type)}
            onChange={(e) => props.edit({ type: Number(e.currentTarget.value) })}
          >
            {TYPES.map(([v, label]) => (
              <option value={v} selected={Number(v) === props.hopper.type}>
                {v} · {label}
              </option>
            ))}
          </select>
        </label>
        <label class="adm-field">
          <span>Status</span>
          <select
            value={String(props.hopper.status)}
            onChange={(e) => props.edit({ status: Number(e.currentTarget.value) })}
          >
            {STATUSES.map(([v, label]) => (
              <option value={v} selected={v === props.hopper.status}>
                {label}
              </option>
            ))}
          </select>
        </label>
      </div>
      <div class="adm-row">
        <span class="adm-field-label">Range fields</span>
        <Index each={props.hopper.ranges}>
          {(r, i) => (
            <input
              class="adm-num"
              type="number"
              min={0}
              title={rangeLabels(props.hopper.type)[i]}
              aria-label={rangeLabels(props.hopper.type)[i]}
              value={r()}
              onInput={(e) => {
                const n = num(e.currentTarget.value);
                if (n !== null) {
                  const ranges = [...props.hopper.ranges] as [number, number, number];
                  ranges[i] = n;
                  props.edit({ ranges });
                }
              }}
            />
          )}
        </Index>
      </div>
      <div class="adm-raw-scroll">
        <table class="adm-records">
          <thead>
            <tr>
              <th />
              <th>Map ID</th>
              <th class="adm-grow">String</th>
              <th>Flags</th>
              <th>Weight</th>
              <th>Word 2</th>
              <th />
            </tr>
          </thead>
          <tbody>
            <Index each={props.hopper.records}>
              {(r, i) => (
                <tr>
                  <td class="adm-rec-idx">{i + 1}</td>
                  <td>
                    <input
                      class="adm-num"
                      type="number"
                      min={0}
                      value={r().id}
                      onInput={(e) => {
                        const n = num(e.currentTarget.value);
                        if (n !== null) props.editRecord(i, { id: n });
                      }}
                    />
                  </td>
                  <td>
                    <input
                      value={r().string}
                      maxLength={MAX_STRING}
                      disabled={!!r().pick}
                      onInput={(e) => props.editRecord(i, { string: e.currentTarget.value })}
                    />
                  </td>
                  <td>
                    <input
                      class="adm-num"
                      type="number"
                      min={0}
                      value={r().flags}
                      disabled={!!r().pick}
                      onInput={(e) => {
                        const n = num(e.currentTarget.value);
                        if (n !== null) props.editRecord(i, { flags: n });
                      }}
                    />
                  </td>
                  <Index each={r().dwords}>
                    {(d, j) => (
                      <td>
                        <input
                          class="adm-num"
                          type="number"
                          min={0}
                          value={d()}
                          disabled={j === 1 && !!r().pick}
                          onInput={(e) => {
                            const n = num(e.currentTarget.value);
                            if (n !== null) {
                              const dwords = [...r().dwords] as [number, number];
                              dwords[j] = n;
                              props.editRecord(i, { dwords });
                            }
                          }}
                        />
                      </td>
                    )}
                  </Index>
                  <td class="adm-rec-btns">
                    <button aria-label="Move up" disabled={i === 0} onClick={() => props.moveRecord(i, -1)}>
                      ↑
                    </button>
                    <button
                      aria-label="Move down"
                      disabled={i === props.hopper.records.length - 1}
                      onClick={() => props.moveRecord(i, 1)}
                    >
                      ↓
                    </button>
                    <button aria-label="Duplicate" disabled={props.hopper.records.length >= MAX_RECORDS} onClick={() => props.copyRecord(i)}>
                      ⧉
                    </button>
                    <button
                      aria-label="Remove"
                      disabled={props.hopper.records.length <= 1}
                      onClick={() => props.removeRecord(i)}
                    >
                      ×
                    </button>
                  </td>
                </tr>
              )}
            </Index>
          </tbody>
        </table>
      </div>
    </details>
  );
}
