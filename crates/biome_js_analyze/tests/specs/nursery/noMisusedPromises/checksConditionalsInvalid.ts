const promise = Promise.resolve('value');

if (promise) {
  // Do something
}

const val = promise ? 123 : 456;

[1, 2, 3].filter(() => promise);

while (promise) {
  // Do something
}

const a = (): boolean | Promise<boolean> => {
  return Promise.resolve(true);
}

if (a()) {
  // Do something
}

while (a()) {
  // Do something
}

declare const reqCache: Required<{p?: Promise<string>}>;
if (reqCache.p) console.log("cached");
const v = reqCache.p ? "yes" : "no";

declare const roCache: Readonly<{p: Promise<string>}>;
if (roCache.p) console.log("cached");
