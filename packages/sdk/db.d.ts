/** @s2script/db — engine-generic async SQLite database. NO runtime code (injected as __s2pkg_db). */

/** A value bindable as a SQL parameter or returned in a {@link Row}. */
export type SqlValue = string | number | boolean | null;
/** One query result row — a column-name→{@link SqlValue} map. */
export type Row = Record<string, SqlValue>;
/** The outcome of a non-SELECT statement run via {@link Database.execute}. */
export interface ExecuteResult {
  /** Rows affected by the statement (INSERT/UPDATE/DELETE). */
  changes: number;
  /** Rowid of the last inserted row (0 if the statement inserted nothing). */
  lastInsertId: number;
}

/** A driver-owned connection — what a {@link Driver} hands back from {@link Driver.connect}. */
export interface DriverConnection {
  /** Run a SELECT and resolve every result {@link Row}; `params` bind `?` placeholders. */
  query(sql: string, params?: SqlValue[]): Promise<Row[]>;
  /** Run a non-SELECT statement and resolve its {@link ExecuteResult}; `params` bind `?` placeholders. */
  execute(sql: string, params?: SqlValue[]): Promise<ExecuteResult>;
  /** Close the underlying connection. */
  close(): Promise<void>;
}
/** Named-connection config resolved by {@link Database.open} and passed to {@link Driver.connect}. */
export interface ConnectionConfig {
  /** Driver name to connect with (e.g. `"sqlite"`). */
  driver: string;
  /** The connection's configured name. */
  name: string;
  /** Driver-specific options (path, host, credentials, …). */
  [k: string]: unknown;
}
/** A registered database backend; register a custom one via {@link Database.registerDriver}. */
export interface Driver {
  /** The driver's name, matched against {@link ConnectionConfig.driver}. */
  readonly name: string;
  /** Open a connection for `config`, resolving a {@link DriverConnection}. */
  connect(config: ConnectionConfig): Promise<DriverConnection>;
}
/** A live database connection (delegates to its driver). */
export interface Database {
  /** Run a SELECT and resolve every result {@link Row}; `params` bind `?` placeholders. */
  query(sql: string, params?: SqlValue[]): Promise<Row[]>;
  /** Run a non-SELECT statement and resolve its {@link ExecuteResult}; `params` bind `?` placeholders. */
  execute(sql: string, params?: SqlValue[]): Promise<ExecuteResult>;
  /** Close the connection. */
  close(): Promise<void>;
}
/** Entry point for opening databases and registering drivers. */
export declare const Database: {
  /**
   * Open a connection by name (default `"default"`). Resolves the driver + config, then connects.
   * @param name - Connection name from config; omit for `"default"`.
   * @returns Resolves the connected {@link Database} handle.
   * @throws Rejects if the name is unknown or the driver fails to connect.
   * @example
   * import { Database } from "@s2script/sdk/db";
   * const db = await Database.open("clientprefs");
   * await db.execute("CREATE TABLE IF NOT EXISTS cookies (steamid TEXT, name TEXT, value TEXT)");
   * const rows = await db.query("SELECT value FROM cookies WHERE steamid = ?", ["76561199999999999"]);
   */
  open(name?: string): Promise<Database>;
  /** Register a custom driver (per-plugin context). SQLite is built in. */
  registerDriver(driver: Driver): void;
};
