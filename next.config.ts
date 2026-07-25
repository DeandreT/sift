import type { NextConfig } from "next";

const githubPages = process.env.GITHUB_ACTIONS === "true";

const nextConfig: NextConfig = {
  output: "export",
  trailingSlash: true,
  turbopack: {
    root: process.cwd(),
  },
  env: {
    NEXT_PUBLIC_BASE_PATH: githubPages ? "/sift" : "",
  },
  images: {
    unoptimized: true,
  },
  ...(githubPages
    ? {
        basePath: "/sift",
        assetPrefix: "/sift/",
      }
    : {}),
};

export default nextConfig;
