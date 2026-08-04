export type U64Input = bigint | number;

export type SensitiveKind =
  | "url"
  | "query"
  | "etag"
  | "topic"
  | "entity_path"
  | "store_id"
  | "internal_token";

export type PublicEffectKind =
  | "event"
  | "store"
  | "route"
  | "selection"
  | "panel"
  | "subscriber"
  | "cache"
  | "callback"
  | "fetch"
  | "connection"
  | "mutation"
  | "query"
  | "animation_frame"
  | "dom_handler"
  | "observer";

export const sensitiveKinds: readonly SensitiveKind[];
export const publicEffectKinds: readonly PublicEffectKind[];

export interface ResourceDefinition {
  key: number;
  maxCount: U64Input;
  maxBytes: U64Input;
}

export interface ReservationRequest {
  key: number;
  count: U64Input;
  bytes: U64Input;
}

export interface ResourceUsageSnapshot {
  readonly currentCount: bigint;
  readonly currentBytes: bigint;
  readonly highWaterCount: bigint;
  readonly highWaterBytes: bigint;
}

export interface ResourceSnapshotEntry {
  readonly key: number;
  readonly maxCount: bigint;
  readonly maxBytes: bigint;
  readonly usage: ResourceUsageSnapshot;
}

export interface ResourceRegistrySnapshot {
  readonly revision: bigint;
  readonly wrapperRetention: {
    readonly prepared: bigint;
    readonly reservations: bigint;
  };
  readonly entries: readonly ResourceSnapshotEntry[];
}

export interface ResourceReservation {
  release(): void;
  dispose(): void;
}

export interface PreparedReservations {
  commit(): ResourceReservation;
  abort(): void;
  dispose(): void;
}

export class CheckedResourceRegistry {
  constructor(resources: Iterable<ResourceDefinition>);
  snapshot(): ResourceRegistrySnapshot;
  prepare(requests: Iterable<ReservationRequest>): PreparedReservations;
}

export function registrySnapshotsEqual(
  left: ResourceRegistrySnapshot,
  right: ResourceRegistrySnapshot,
): boolean;

export function assertRegistryUnchanged(
  before: ResourceRegistrySnapshot,
  after: ResourceRegistrySnapshot,
): void;

export interface ExactSnapshotCallbacks<Value, Snapshot> {
  clone(value: Value): Snapshot;
  equals(snapshot: Snapshot, actual: Value): boolean;
}

export class ExactSnapshot<Value, Snapshot> {
  private constructor();
  static capture<Value, Snapshot>(
    value: Value,
    callbacks: ExactSnapshotCallbacks<Value, Snapshot>,
  ): ExactSnapshot<Value, Snapshot>;
  isUnchanged(actual: Value): boolean;
  assertUnchanged(actual: Value): void;
  dispose(): void;
}

export class LeakDetector {
  register(
    kind: SensitiveKind,
    value: string | Uint8Array | ArrayBuffer,
  ): this;
  inspect(observable: string | Uint8Array | ArrayBuffer): void;
  clone(): LeakDetector;
  dispose(): void;
  get size(): number;
}

export function parseRedactionLeakCorpusV1(text: string): LeakDetector;

export function boundedLabel(
  value: string,
  maxBytes: number,
  detector: LeakDetector,
): string;

export function checkZeroized(value: Uint8Array | ArrayBuffer): void;

export interface ZeroizeObserver {
  check(): void;
}

export interface ZeroizeInspection {
  readonly byteLength: number;
  at(index: number): number;
  set(index: number, value: number): void;
  fill(value: number): void;
  isZeroized(): boolean;
}

export class ZeroizingTestBytes {
  constructor(value: string | Uint8Array | ArrayBuffer);
  inspect<Result>(callback: (bytes: ZeroizeInspection) => Result): Result;
  dispose(): void;
  observer(): ZeroizeObserver;
}

export interface PublicEffectSnapshotEntry {
  readonly kind: PublicEffectKind;
  readonly count: bigint;
}

export type PublicEffectSnapshot = readonly PublicEffectSnapshotEntry[];

export class PublicEffectProbe {
  record(kind: PublicEffectKind): void;
  snapshot(): PublicEffectSnapshot;
}

export function assertNoPublicEffects(
  before: PublicEffectSnapshot,
  after: PublicEffectSnapshot,
): void;

export function captureInternerSnapshot(
  readBytesUsed: () => U64Input,
): bigint;

export function assertNoInternerDelta(
  before: bigint,
  after: bigint,
): void;
