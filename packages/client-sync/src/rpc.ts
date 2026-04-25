export interface RpcClient {
  call<R = unknown, P = unknown>(method: string, params?: P, options?: { scopeId?: string | null }): Promise<R>;
}

let activeRpcClient: RpcClient | null = null;

export function installRpcClient(client: RpcClient) {
  activeRpcClient = client;
}

export function getRpcClient() {
  if (!activeRpcClient) {
    throw new Error("RPC client not installed");
  }
  return activeRpcClient;
}

export async function callRpc<R = unknown, P = unknown>(method: string, params?: P, options?: { scopeId?: string | null }) {
  return getRpcClient().call<R, P>(method, params, options);
}
