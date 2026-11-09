import type { Hook } from "@s2script/sdk/interfaces";

/** Coordination for plugins that own or choose a map change, such as RTV and mapchooser. */
export interface Contract {
  methods: {
    /**
     * Set the map the next automatic rotation goes to (the `sm_setnextmap` override, and the
     * `nextlevel` cvar). `map` is resolved against maplist.txt first, so a workshop entry keeps
     * its id, then against the installed maps. Returns false when the map is unknown or invalid.
     */
    setNextMap(map: string): boolean;
  };
  forwards: {
    /** Return Handled to suppress this automatic rotation. Startup switches bypass this hook. */
    OnAutoMapChange: Hook<{ currentMap: string; nextMap: string }>;
  };
}
