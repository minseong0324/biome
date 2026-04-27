---
"@biomejs/biome": patch
---

[`noMisleadingReturnType`](https://biomejs.dev/linter/rules/no-misleading-return-type/) no longer reports false positives when a union return type's `boolean` variant is covered by both `true` and `false` returns.

[`useExhaustiveSwitchCases`](https://biomejs.dev/linter/rules/use-exhaustive-switch-cases/) now flags missing `true`/`false` cases for `boolean` discriminants, including when `boolean` is a union variant.

```ts
const useCloudLogin = (
    authenticated: boolean,
    a: boolean,
    b: boolean,
): boolean | null => {
    if (!authenticated) {
        if (a) return true;
        if (b) return false;
    }
    return null;
};
```
