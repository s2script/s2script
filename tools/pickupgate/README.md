# pickupgate

Live-gate fixture for `ctx.items.onCanAcquire` / `onCanAcquirePost`. Not shipped.

Prefix: `[PICKUPGATE]`. Drive with `python3 scripts/rcon.py pickup_report` / `pickup_deny` / `pickup_give` / `pickup_reenter`.

Checks (spec §8):

1. Natural pickup fires the handler.
2. `pickup_give` (`giveNamedItem`) fires the handler.
3. `pickup_deny` then a give withholds the item (`Handled` + `InvalidItem`).
4. `pickup_reenter` then a give: the inner `giveNamedItem` is skipped and named.
5. With this plugin removed, boot log shows the detour was never installed.
6. After a denied give, Post logs `skipped=true`.
