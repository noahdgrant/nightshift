declare const test: { skip(condition: boolean, reason: string): void };
declare const isCI: boolean;
declare const browserName: string;

test.skip(isCI, "flaky on CI");
test.skip(browserName === "firefox", "not on firefox");
