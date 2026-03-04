import type { ComponentChildren } from "preact";

interface HeaderProps {
  children?: ComponentChildren;
}

export function Header({ children }: HeaderProps) {
  return (
    <header class="border-b border-sumi-ink3 px-4 py-3 flex items-center justify-between">
      <div class="flex items-center gap-3">
        <h1 class="text-lg font-semibold tracking-tight text-fuji-white">
          RSS Centr
        </h1>
      </div>
      {children && <div class="flex items-center gap-2">{children}</div>}
    </header>
  );
}
