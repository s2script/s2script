import { Database } from "@s2script/sdk/db";

export async function OnPluginStart(): Promise<void> {
  Database.open("prefs");
}
