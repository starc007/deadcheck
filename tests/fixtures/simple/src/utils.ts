export function greet(name: string): string {
    return `Hello, ${name}!`;
}

// This export is never used by anyone.
export function legacyHelper(): void {}
