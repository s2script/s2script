// Hand-written ambient type for the `zones` inter-plugin interface (published by @s2script/zones).
// Interface .d.ts codegen is deferred; the consumer declares the producer's shape by hand (mirroring
// the greeter-consumer pattern), plus the runtime-injected on/off event API every consumed proxy carries.
declare module "zones" {
  interface Vec3 { x: number; y: number; z: number; }
  interface ZoneEvent { zone: string; slot: number; userId: number; }
  interface Zones {
    createZone(name: string, min: Vec3, max: Vec3): boolean;
    deleteZone(name: string): boolean;
    getZones(): { name: string; min: Vec3; max: Vec3 }[];
    isInZone(slot: number, name: string): boolean;
    zonesFor(slot: number): string[];
    on(event: "enter" | "leave" | "stay", handler: (p: ZoneEvent) => void): number;
    off(event: string, handler: (...args: any[]) => void): void;
  }
  const _default: Zones;
  export = _default;
}
