import type { ComponentChildren } from "preact";

export interface ButtonProps {
  id?: string;
  type?: "button" | "submit" | "reset";
  onClick?: () => void;
  children?: ComponentChildren;
  disabled?: boolean;
  variant?: "primary" | "secondary" | "ghost";
}

export function Button(
  { variant = "secondary", ...props }: ButtonProps,
) {
  const base = "px-3 py-1.5 text-sm rounded-md transition-colors font-medium";
  const variants = {
    primary: "bg-amber-600 text-white hover:bg-amber-500",
    secondary:
      "border border-neutral-700 text-neutral-300 hover:bg-neutral-800",
    ghost: "text-neutral-400 hover:text-neutral-200 hover:bg-neutral-800",
  };

  return (
    <button
      {...props}
      class={`${base} ${variants[variant]} ${
        props.disabled ? "opacity-50 cursor-not-allowed" : ""
      }`}
    />
  );
}
