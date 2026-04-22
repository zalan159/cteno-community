export class AsyncMessageQueue<T> implements AsyncIterable<T> {
  private readonly items: T[] = [];
  private readonly resolvers: Array<(result: IteratorResult<T>) => void> = [];
  private closed = false;

  push(item: T): void {
    if (this.closed) {
      throw new Error('Cannot push to a closed AsyncMessageQueue');
    }

    const resolver = this.resolvers.shift();
    if (resolver) {
      resolver({ done: false, value: item });
      return;
    }

    this.items.push(item);
  }

  close(): void {
    if (this.closed) {
      return;
    }

    this.closed = true;
    while (this.resolvers.length > 0) {
      const resolver = this.resolvers.shift();
      resolver?.({ done: true, value: undefined as T });
    }
  }

  [Symbol.asyncIterator](): AsyncIterator<T> {
    return {
      next: () => {
        if (this.items.length > 0) {
          const value = this.items.shift() as T;
          return Promise.resolve({ done: false, value });
        }

        if (this.closed) {
          return Promise.resolve({ done: true, value: undefined as T });
        }

        return new Promise<IteratorResult<T>>(resolve => {
          this.resolvers.push(resolve);
        });
      },
    };
  }
}
