import { http, type RequestOptions } from '../http'
import type { EvalFeedbackBody, EvalFeedbackRun, EvalHistoryResponse, EvalQuestion } from '../types'

/** Retrieval evaluation: benchmark history and real-question feedback. */
export const evalApi = {
  /** Benchmark runs in append order (oldest first). */
  evalHistory: (limit: number, options?: RequestOptions) =>
    http.get<EvalHistoryResponse>(`/v1/eval/history?limit=${limit}`, options),
  /** Saved real questions, paged by offset. */
  evalFeedback: (offset: number) =>
    http.get<{ items: EvalQuestion[]; total: number }>(`/v1/eval/feedback?offset=${offset}`),
  /** Save a question with its expected sources. Answers 204. */
  saveEvalFeedback: (body: EvalFeedbackBody) => http.postOptional('/v1/eval/feedback', body),
  /** Re-run one saved question against the current corpus. */
  runEvalFeedback: (id: string) => http.post<EvalFeedbackRun>('/v1/eval/feedback/run', { id }),
}
