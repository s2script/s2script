import type { Hook } from "@s2script/sdk/interfaces";

/** Coordination for plugins that own a pending map change, such as RTV. */
export interface Contract {
  methods: {};
  forwards: {
    /** Return Handled to suppress this automatic rotation. Startup switches bypass this hook. */
    OnAutoMapChange: Hook<{ currentMap: string; nextMap: string }>;
  };
}
