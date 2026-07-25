import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "sift | Azure Service Bus, in full view",
  description:
    "A native, open-source Azure Service Bus explorer for namespace topology, message inspection, and deliberate operations.",
  applicationName: "sift",
  openGraph: {
    title: "sift | Azure Service Bus, in full view",
    description:
      "Inspect namespaces, messages, sessions, dead-letter queues, and runtime state from one native Rust workspace.",
    type: "website",
    images: [
      {
        url: "https://raw.githubusercontent.com/DeandreT/sift/main/public/sift-connect.png",
        width: 1280,
        height: 800,
        alt: "The sift Azure Service Bus explorer connection workspace",
      },
    ],
  },
  icons: {
    icon: [
      {
        url: "data:image/svg+xml,<svg xmlns=%22http://www.w3.org/2000/svg%22 viewBox=%220 0 32 32%22><rect width=%2232%22 height=%2232%22 rx=%225%22 fill=%22%23141616%22/><path d=%22M7 8h18l-7 8v6l-4 2v-8z%22 fill=%22%236ee7f2%22/></svg>",
        type: "image/svg+xml",
      },
    ],
  },
};

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}
