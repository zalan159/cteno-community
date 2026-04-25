import type { Message } from "./messages";

export interface MessageState {
  messages: Message[];
  byId: Record<string, Message>;
}

export type MessageAction =
  | { type: "replace"; messages: Message[] }
  | { type: "append"; message: Message }
  | { type: "update"; id: string; patch: Partial<Message> }
  | { type: "remove"; id: string };

export function createMessageState(messages: Message[] = []): MessageState {
  return {
    messages,
    byId: Object.fromEntries(messages.map((message) => [message.id, message])),
  };
}

export function messageReducer(state: MessageState, action: MessageAction): MessageState {
  switch (action.type) {
    case "replace":
      return createMessageState(action.messages);
    case "append":
      return createMessageState([...state.messages, action.message]);
    case "update":
      return createMessageState(
        state.messages.map((message) =>
          message.id === action.id ? ({ ...message, ...action.patch } as Message) : message,
        ),
      );
    case "remove":
      return createMessageState(state.messages.filter((message) => message.id !== action.id));
    default:
      return state;
  }
}

export class MessageCache {
  private state = createMessageState();

  replace(messages: Message[]) {
    this.state = createMessageState(messages);
  }

  append(message: Message) {
    this.state = messageReducer(this.state, { type: "append", message });
  }

  get(id: string) {
    return this.state.byId[id] ?? null;
  }

  list() {
    return this.state.messages;
  }
}
