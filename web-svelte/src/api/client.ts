/**
 * Gateway client facade.
 *
 * - `./http` is the transport: auth token, error mapping, JSON parsing.
 * - `./endpoints/*` are the typed endpoint groups; every path lives there.
 * - `api` flattens the groups so screens and stores call `api.status()` and never
 *   spell a `/v1/...` path or an inline response type themselves.
 */
import { agentsApi } from './endpoints/agents'
import { corpusApi } from './endpoints/corpus'
import { evalApi } from './endpoints/eval'
import { graphApi } from './endpoints/graph'
import { searchApi } from './endpoints/search'
import { systemApi } from './endpoints/system'
import { wikiApi } from './endpoints/wiki'

export {
  ApiError,
  apiUrl,
  hasGatewayToken,
  http,
  parseJsonBody,
  parseOptionalJsonBody,
  setGatewayToken,
  type RequestOptions,
} from './http'
export { loadPanels } from '@/lib/async'
export { documentsQuery } from './endpoints/corpus'
export { wikiListQuery } from './endpoints/wiki'

export const api = {
  ...systemApi,
  ...agentsApi,
  ...corpusApi,
  ...searchApi,
  ...wikiApi,
  ...graphApi,
  ...evalApi,
}
