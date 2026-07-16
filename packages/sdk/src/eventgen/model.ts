// Pure model: event-catalog → typed event descriptors.
// No I/O, no Date/random — deterministic. Sort events + field lists alphabetically.

export type EventCatalog = Record<string, Record<string, string>>;

export type GetterKey = "getBool" | "getFloat" | "getInt" | "getPlayerSlot" | "getString" | "getUint64";

export interface EventDescriptor {
  event: string;
  iface: string;
  byGetter: { [K in GetterKey]: string[] };
}

const TYPE_TO_GETTER: Record<string, GetterKey> = {
  bool: "getBool",
  float: "getFloat",
  int: "getInt",
  player: "getPlayerSlot",
  string: "getString",
  uint64: "getUint64",
};

/** PascalCase("player_death") → "PlayerDeath" (then caller appends "Event"). */
function pascalCase(name: string): string {
  return name.split("_").map((w) => w.charAt(0).toUpperCase() + w.slice(1)).join("");
}

/** Build a sorted, deterministic model from an event catalog. */
export function buildEventModel(catalog: EventCatalog): EventDescriptor[] {
  return Object.keys(catalog).sort().map((event) => {
    const fields = catalog[event];
    const byGetter: { [K in GetterKey]: string[] } = {
      getBool: [],
      getFloat: [],
      getInt: [],
      getPlayerSlot: [],
      getString: [],
      getUint64: [],
    };
    for (const field of Object.keys(fields).sort()) {
      const type = fields[field];
      const getter = TYPE_TO_GETTER[type];
      if (getter) byGetter[getter].push(field);
    }
    return {
      event,
      iface: pascalCase(event) + "Event",
      byGetter,
    };
  });
}
