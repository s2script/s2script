/** @s2script/db — engine-generic async SQLite database. NO runtime code (injected as __s2pkg_db). */
export type SqlValue = string | number | boolean | null;
export type Row = Record<string, SqlValue>;
export interface ExecuteResult { changes: number; lastInsertId: number; }

export interface DriverConnection {
  query(sql: string, params?: SqlValue[]): Promise<Row[]>;
  execute(sql: string, params?: SqlValue[]): Promise<ExecuteResult>;
  close(): Promise<void>;
}
export interface ConnectionConfig { driver: string; name: string; [k: string]: unknown; }
export interface Driver {
  readonly name: string;
  connect(config: ConnectionConfig): Promise<DriverConnection>;
}
/** A live database connection (delegates to its driver). */
export interface Database {
  query(sql: string, params?: SqlValue[]): Promise<Row[]>;
  execute(sql: string, params?: SqlValue[]): Promise<ExecuteResult>;
  close(): Promise<void>;
}
export declare const Database: {
  /** Open a connection by name (default "default"). Resolves the driver + config, then connects. */
  open(name?: string): Promise<Database>;
  /** Register a custom driver (per-plugin context). SQLite is built in. */
  registerDriver(driver: Driver): void;
};
