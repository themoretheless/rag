import { http, type RequestOptions } from '../http'
import type {
  Capabilities,
  DoctorReport,
  EmbeddingManifestStatus,
  HealthResponse,
  LlmStatus,
  ResourcesResponse,
  RuntimeInfo,
  StorageStatus,
} from '../types'

/** Gateway health, storage, runtime and maintenance operations. */
export const systemApi = {
  // The shell polls this: skip the server's integrity scans (see /v1/doctor).
  health: (options?: RequestOptions) => http.get<HealthResponse>('/health?summary=true', options),
  resources: (options?: RequestOptions) => http.get<ResourcesResponse>('/v1/resources', options),

  /** Full storage status (counts, index coverage, embedder identity). */
  status: (options?: RequestOptions) => http.get<StorageStatus>('/v1/status', options),
  /** Cheap status subset for screens that poll. */
  statusSummary: (options?: RequestOptions) => http.get<StorageStatus>('/v1/status?summary=true', options),
  /** Integrity scan; noticeably slower than status. */
  doctor: (options?: RequestOptions) => http.get<DoctorReport>('/v1/doctor', options),
  runtime: async (options?: RequestOptions): Promise<RuntimeInfo> =>
    (await http.get<{ runtime?: RuntimeInfo }>('/v1/runtime', options)).runtime ?? {},
  capabilities: (options?: RequestOptions) => http.get<Capabilities>('/v1/capabilities', options),
  llmStatus: async (options?: RequestOptions): Promise<LlmStatus> =>
    (await http.get<{ llm?: LlmStatus }>('/v1/llm-status', options)).llm ?? {},
  embeddingManifest: (options?: RequestOptions) =>
    http.get<EmbeddingManifestStatus>('/v1/embedding-manifest', options),

  /** Fold the WAL into the database file. Answers 204. */
  checkpoint: () => http.postOptional('/v1/operations/checkpoint'),
  /** Write a backup to a server-side path; never overwrites an existing file. */
  backup: (path: string) =>
    http.postOptional('/v1/operations/backup', { path, dry_run: false, overwrite: false }),
}
