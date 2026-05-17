import { afterEach, describe, expect, it, vi } from "vitest";
import {
  fetchPost,
  fetchPosts,
  slugify,
  titleCase,
  truncate,
  wordCount,
} from "./utils";

describe("slugify", () => {
  it("lowercases and dashes spaces", () => {
    expect(slugify("Hello World")).toBe("hello-world");
  });

  it("strips punctuation", () => {
    expect(slugify("It's a Test!")).toBe("its-a-test");
  });

  it("collapses runs of whitespace and dashes", () => {
    expect(slugify("  multiple   spaces  ")).toBe("multiple-spaces");
    expect(slugify("dash--dash")).toBe("dash-dash");
  });

  it("returns an empty string for empty input", () => {
    expect(slugify("")).toBe("");
    expect(slugify("   ")).toBe("");
  });
});

describe("titleCase", () => {
  it("capitalises each word", () => {
    expect(titleCase("hello world")).toBe("Hello World");
  });

  it("collapses redundant whitespace", () => {
    expect(titleCase("  vitest   plays   nicely  ")).toBe(
      "Vitest Plays Nicely",
    );
  });

  it("handles single-character words", () => {
    expect(titleCase("a b c")).toBe("A B C");
  });
});

describe("wordCount", () => {
  it("counts words separated by whitespace", () => {
    expect(wordCount("one two three")).toBe(3);
  });

  it("returns 0 for empty or whitespace-only input", () => {
    expect(wordCount("")).toBe(0);
    expect(wordCount("    ")).toBe(0);
  });

  it("treats tabs and newlines as separators", () => {
    expect(wordCount("one\ttwo\nthree")).toBe(3);
  });
});

describe("truncate", () => {
  it("returns the input unchanged when it fits", () => {
    expect(truncate("short", 10)).toBe("short");
  });

  it("appends an ellipsis when truncating", () => {
    expect(truncate("hello world", 8)).toBe("hello w…");
  });

  it("degrades gracefully for very small limits", () => {
    expect(truncate("hello", 1)).toBe("h");
    expect(truncate("hello", 0)).toBe("");
  });
});

describe("fetchPost", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("returns the parsed body on a 200", async () => {
    const post = { userId: 1, id: 1, title: "t", body: "b" };
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        json: async () => post,
      }),
    );
    await expect(fetchPost(1)).resolves.toEqual(post);
  });

  it("throws when the response is not ok", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        ok: false,
        status: 404,
        json: async () => ({}),
      }),
    );
    await expect(fetchPost(9999)).rejects.toThrow("HTTP 404");
  });
});

describe("fetchPosts", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("returns the parsed array on a 200", async () => {
    const posts = [{ userId: 1, id: 1, title: "t", body: "b" }];
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        json: async () => posts,
      }),
    );
    await expect(fetchPosts()).resolves.toEqual(posts);
  });
});
