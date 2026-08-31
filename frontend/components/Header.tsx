import type { ComponentChildren } from "preact";

interface HeaderProps {
  children?: ComponentChildren;
}

export function Header({ children }: HeaderProps) {
  return (
    <header class="flex flex-col gap-2 border-b border-sumi-ink3 px-3 py-3 sm:flex-row sm:items-center sm:justify-between sm:px-4">
      <div class="flex items-center gap-3">
        <h1 class="whitespace-nowrap text-lg font-semibold tracking-tight text-fuji-white">
          RSS Centr
        </h1>
      </div>
      {children && (
        <nav class="flex w-full items-center justify-between gap-1 sm:w-auto sm:justify-start sm:gap-2">
          {children}
        </nav>
      )}
    </header>
  );
}
