import type { Notification } from "@s2script/sdk/interfaces";
export interface Contract {
  methods: {};
  forwards: {
    OnVector: Notification<{
      "😀": 0.000001;
      "\uE000": 100000000000000000000;
      small: 0.0000001;
      large: 1e21;
      fraction: 1.2345678901234567;
      negativeZero: -0;
    }>;
  };
}
