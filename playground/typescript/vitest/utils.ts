export interface Post {
  userId: number;
  id: number;
  title: string;
  body: string;
}

const PLACEHOLDER = "https://jsonplaceholder.typicode.com";

export async function fetchPost(id: number): Promise<Post> {
  const res = await fetch(`${PLACEHOLDER}/posts/${id}`);
  if (!res.ok) {
    throw new Error(`HTTP ${res.status}`);
  }
  return (await res.json()) as Post;
}

export async function fetchPosts(): Promise<Post[]> {
  const res = await fetch(`${PLACEHOLDER}/posts`);
  if (!res.ok) {
    throw new Error(`HTTP ${res.status}`);
  }
  return (await res.json()) as Post[];
}

export function slugify(input: string): string {
  return input
    .toLowerCase()
    .trim()
    .replace(/[^a-z0-9\s-]/g, "")
    .replace(/\s+/g, "-")
    .replace(/-+/g, "-");
}

export function titleCase(input: string): string {
  return input
    .toLowerCase()
    .split(/\s+/)
    .filter((w) => w.length > 0)
    .map((w) => w[0].toUpperCase() + w.slice(1))
    .join(" ");
}

export function wordCount(input: string): number {
  const trimmed = input.trim();
  if (trimmed.length === 0) return 0;
  return trimmed.split(/\s+/).length;
}

export function truncate(input: string, max: number): string {
  if (input.length <= max) return input;
  if (max <= 1) return input.slice(0, max);
  return input.slice(0, max - 1) + "…";
}
