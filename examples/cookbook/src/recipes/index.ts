import type { Recipe } from "../recipe.ts";
import { clientsRecipe } from "./clients.ts";
import { cookiesRecipe } from "./cookies.ts";
import { dbRecipe } from "./db.ts";
import { eventsRecipe } from "./events.ts";
import { gamerulesRecipe } from "./gamerules.ts";
import { httpRecipe } from "./http.ts";
import { itemsRecipe } from "./items.ts";
import { menuRecipe } from "./menu.ts";
import { netRecipe } from "./net.ts";
import { playerStateRecipe } from "./player-state.ts";
import { serverRecipe } from "./server.ts";
import { soundRecipe } from "./sound.ts";
import { teamRecipe } from "./team.ts";
import { traceRecipe } from "./trace.ts";
import { translationsRecipe } from "./translations.ts";
import { transmitRecipe } from "./transmit.ts";
import { usercmdRecipe } from "./usercmd.ts";
import { usermessagesRecipe } from "./usermessages.ts";
import { wsRecipe } from "./ws.ts";
import { zonesRecipe } from "./zones.ts";

/** Every recipe the cookbook registers. Add new ones here. */
export const RECIPES: readonly Recipe[] = [
  clientsRecipe,
  cookiesRecipe,
  dbRecipe,
  eventsRecipe,
  gamerulesRecipe,
  httpRecipe,
  itemsRecipe,
  menuRecipe,
  netRecipe,
  playerStateRecipe,
  serverRecipe,
  soundRecipe,
  teamRecipe,
  traceRecipe,
  translationsRecipe,
  transmitRecipe,
  usercmdRecipe,
  usermessagesRecipe,
  wsRecipe,
  zonesRecipe,
];
