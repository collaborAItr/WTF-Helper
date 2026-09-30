/**
 * WTF Helper protocol, version 1.
 *
 * This file is the published contract between the WTF Helper desktop app and a
 * service that relays checks to it. It has no runtime dependencies so a service
 * can copy it as-is. Prose, routes and error codes: PROTOCOL.md next to this file.
 *
 * Licensed under the Apache License, Version 2.0. See LICENSE at the repo root.
 */

export const WTF_HELPER_PROTOCOL_VERSION = 1;

/** The only checks a helper runs. Anything else is refused by the service and by the helper. */
export const WTF_HELPER_CHECKS = [
  'system_overview',
  'disk_space',
  'network_status',
  'security_status',
  'heavy_programs',
  'startup_items',
  'recent_crashes',
  'installed_apps',
] as const;

export type WtfHelperCheckName = (typeof WTF_HELPER_CHECKS)[number];

/** Who approved the check: the person on the check card, or "Allow all checks for this fix". */
export type WtfHelperApproval = 'person' | 'allow_all';

/** Checks whose rows the person previews and can untick before anything reaches the guide. */
export const WTF_HELPER_LIST_CHECKS: readonly WtfHelperCheckName[] = [
  'heavy_programs',
  'startup_items',
  'recent_crashes',
  'installed_apps',
];

/** At most this many checks per request. Each name at most once. */
export const WTF_HELPER_MAX_CHECKS_PER_REQUEST = 3;

/**
 * Options are whole numbers inside these bounds. No paths, commands, or free text.
 * A check that is not listed here takes no options.
 */
export const WTF_HELPER_OPTION_BOUNDS: Record<
  WtfHelperCheckName,
  Readonly<Record<string, { min: number; max: number }>>
> = {
  system_overview: {},
  disk_space: {},
  network_status: {},
  security_status: {},
  heavy_programs: { limit: { min: 1, max: 25 } },
  startup_items: { limit: { min: 1, max: 100 } },
  recent_crashes: { limit: { min: 1, max: 50 }, windowDays: { min: 1, max: 30 } },
  installed_apps: { limit: { min: 1, max: 200 } },
};

/** Plain label and "what it reads" line for each check, as the helper window shows them. */
export const WTF_HELPER_CHECK_LABELS: Record<WtfHelperCheckName, { label: string; reads: string }> = {
  system_overview: {
    label: 'Computer overview',
    reads: 'The model, system version, processor, memory, how long it has been on, and battery health.',
  },
  disk_space: {
    label: 'Disk space',
    reads: 'How full your drives are. Only totals, no file names.',
  },
  network_status: {
    label: 'Network',
    reads: 'Whether you are connected, your router and DNS addresses, and whether collaborAItr can be reached. Not your Wi-Fi name.',
  },
  security_status: {
    label: 'Security settings',
    reads: 'Whether disk encryption, the firewall and built-in protection are switched on.',
  },
  heavy_programs: {
    label: 'Busy programs',
    reads: 'The names of the programs using the most processor and memory right now.',
  },
  startup_items: {
    label: 'Programs that start by themselves',
    reads: 'The names of programs set to start when you sign in.',
  },
  recent_crashes: {
    label: 'Recent crashes',
    reads: 'The names and times of programs that crashed recently. Not what was in them.',
  },
  installed_apps: {
    label: 'Installed apps',
    reads: 'The names and versions of the apps installed on this computer.',
  },
};

export interface WtfHelperCheckRequest {
  check: WtfHelperCheckName;
  options?: Record<string, number>;
}

// ---------------------------------------------------------------------------
// Pairing
// ---------------------------------------------------------------------------

/** `POST {base}/pair/start` (the person's own session, paid plans). Response `data`. */
export interface WtfHelperPairStartData {
  /** Six digits. */
  code: string;
  /** ISO 8601. Ten minutes after the code was made. */
  expiresAt: string;
}

/** `POST {base}/pair/claim` body, sent by the helper. No other auth. */
export interface WtfHelperPairClaimRequest {
  /** Six digits. */
  code: string;
  /** Up to 80 characters. */
  deviceName: string;
  /** Up to 40 characters, e.g. "macOS" or "Windows". */
  os: string;
  /** Up to 80 characters. Optional. */
  osVersion?: string | null;
  /** Up to 40 characters. */
  helperVersion: string;
}

/** `POST {base}/pair/claim` response `data`. The helper keeps `deviceToken` in the OS keychain. */
export interface WtfHelperPairClaimData {
  deviceId: string;
  deviceToken: string;
  userId: string;
}

// ---------------------------------------------------------------------------
// Stream: `GET {base}/stream` with header `X-WTF-Helper-Token: <deviceToken>`
// Server-sent events. Each event is one `data: <json>` line.
// ---------------------------------------------------------------------------

export interface WtfHelperHeartbeatEvent {
  type: 'heartbeat';
  /** ISO 8601. Sent every 10 seconds. */
  at: string;
}

export interface WtfHelperCheckEvent {
  type: 'check';
  requestId: string;
  /** Opaque, signed by the service. Echo it unchanged with the result. */
  receipt: string;
  approval: WtfHelperApproval;
  /** One to three checks, each name at most once. */
  checks: WtfHelperCheckRequest[];
  /** ISO 8601. After this the service no longer accepts a result. */
  expiresAt: string;
}

export type WtfHelperStreamEvent = WtfHelperHeartbeatEvent | WtfHelperCheckEvent;

// ---------------------------------------------------------------------------
// Result: `POST {base}/checks/{requestId}/result` with `X-WTF-Helper-Token`
// ---------------------------------------------------------------------------

export interface WtfHelperCheckResultItem {
  check: WtfHelperCheckName;
  ok: boolean;
  /** Plain sentence for the guide. Up to 500 characters. */
  summary?: string;
  /** Plain reason when `ok` is false. Up to 200 characters. */
  error?: string;
  /** Structured, redacted output. See CHECKS.md for each check's shape. */
  data?: unknown;
}

export interface WtfHelperResultRequest {
  /** The `receipt` from the check event, unchanged. */
  receipt: string;
  /** The `approval` from the check event, unchanged. */
  approval: WtfHelperApproval;
  /** Exactly one result per requested check. */
  results: WtfHelperCheckResultItem[];
}

/** What the service returns to the web app once the helper answers. */
export interface WtfHelperCheckResponse {
  requestId: string;
  approval: WtfHelperApproval;
  results: WtfHelperCheckResultItem[];
}

/** List checks share this `data` shape so a preview can untick rows. */
export interface WtfHelperListData<Item extends { label: string }> {
  items: Item[];
  /** How many rows existed before `limit` and clipping. */
  total: number;
  /** True if rows were left out because of `limit` or the size cap. */
  truncated: boolean;
}

// ---------------------------------------------------------------------------
// Envelope and errors
// ---------------------------------------------------------------------------

export type WtfHelperEnvelope<T> =
  | { success: true; data?: T }
  | { success: false; error: string; code: WtfHelperErrorCode | string; recoverable?: true };

export type WtfHelperErrorCode =
  | 'not_enabled'
  | 'paid_plan_required'
  | 'token_required'
  | 'invalid_token'
  | 'revoked'
  | 'invalid_code'
  | 'code_used'
  | 'code_expired'
  | 'wrong_user'
  | 'invalid_device'
  | 'approval_required'
  | 'invalid_checks'
  | 'too_many_checks'
  | 'unknown_check'
  | 'invalid_options'
  | 'invalid_receipt'
  | 'request_expired'
  | 'wrong_device'
  | 'invalid_result'
  | 'result_too_large'
  | 'helper_offline'
  | 'try_again'
  | 'not_found'
  | 'rate_limited'
  | 'pair_failed'
  | 'notify_too_large'
  | 'misconfigured'
  | 'relay_failed';
