const anyArray: any[] = [];
anyArray.sort();
anyArray.toSorted();

anyArray.sort(undefined);
anyArray.toSorted(undefined);

const stringArray: string[] = [];
stringArray.sort();
stringArray.toSorted();

declare const pickedNums: Pick<{ nums: number[]; label: string }, "nums">;
pickedNums.nums.sort();
pickedNums.nums.toSorted();

declare const omittedNums: Omit<{ nums: number[]; label: string }, "label">;
omittedNums.nums.sort();
