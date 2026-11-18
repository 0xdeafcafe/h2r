export const concepts = [
  { id: "orbital", name: "Orbital" },
  { id: "silverline", name: "Silverline" },
  { id: "switchboard", name: "Switchboard" },
] as const;
export type Concept = typeof concepts[number]["id"];

export const destinations = [
  { id: "friends", label: "Friends", detail: "Select a friend to see their lobby or last visit." },
  { id: "messages", label: "Messages", detail: "Open a transmission to read the message." },
  { id: "sessions", label: "Sessions", detail: "Check the host and available slots before playing." },
  { id: "account", label: "Account", detail: "Change the name shown to other players." },
  { id: "server", label: "Server / Admin", detail: "Inspect player traffic and completed games." },
] as const;
export type Section = typeof destinations[number]["id"];
export type Screen = "home" | Section;
export function screenFrom(value: string | string[] | undefined): Screen {
  return destinations.find((item) => item.id === value)?.id ?? "home";
}

export const roster = [
  { tag: "Kestrel", state: "Playing", location: "Team doubles · Glasshouse" },
  { tag: "Mossfire", state: "In lobby", location: "Waiting for players · Slipway" },
  { tag: "Halcyon", state: "Playing", location: "Free for all · Meridian" },
  { tag: "Arcminute", state: "Offline", location: "Last seen yesterday" },
  { tag: "Ebb", state: "Offline", location: "Last seen 2 days ago" },
];
export const online = roster.filter((friend) => friend.state !== "Offline");
export const messages = [
  { from: "Mossfire", time: "17:48", subject: "One more for doubles?", body: "I've got a spot in the Slipway lobby if you're around." },
  { from: "Kestrel", time: "17:32", subject: "Good games", body: "That last round on Glasshouse was close. Same time tomorrow?" },
];
export const sessions = [
  { map: "Slipway", mode: "Team doubles", host: "Mossfire", slots: "3 / 4" },
  { map: "Glasshouse", mode: "Team skirmish", host: "Beryl", slots: "6 / 8" },
  { map: "Meridian", mode: "Free for all", host: "Halcyon", slots: "5 / 8" },
  { map: "Causeway", mode: "Capture the flag", host: "Cirrus", slots: "9 / 12" },
];
export const history = [
  ["17:54", "Glasshouse", "Team doubles", 4, "12:08"],
  ["17:41", "Meridian", "Free for all", 8, "09:34"],
  ["17:29", "Slipway", "Team skirmish", 8, "14:52"],
  ["17:16", "Causeway", "Capture the flag", 12, "18:10"],
  ["16:58", "Meridian", "Team doubles", 4, "10:43"],
  ["16:44", "Glasshouse", "Free for all", 6, "11:07"],
] as const;

// Concurrent players sampled every half hour, 00:00–18:00 UTC.
export const traffic = [12, 14, 11, 13, 8, 9, 7, 6, 8, 12, 9, 11, 17, 21, 19, 24, 18, 22, 29,
  32, 26, 28, 33, 31, 38, 43, 35, 39, 51, 47, 53, 58, 48, 52, 46, 40, 42];
export const capacity = 64;
export const players = traffic[traffic.length - 1]!;
export const peak = Math.max(...traffic);

export function sampleTime(index: number): string {
  return String(Math.floor(index / 2)).padStart(2, "0") + (index % 2 ? ":30" : ":00");
}
