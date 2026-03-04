import type { ComponentChildren } from "preact";

export interface ButtonProps {
  id?: string;
  type?: "button" | "submit" | "reset";
  onClick?: () => void;
  children?: ComponentChildren;
  disabled?: boolean;
  variant?: "primary" | "secondary" | "ghost" | "danger";
}

export function Button(
  { variant = "secondary", ...props }: ButtonProps,
) {
  const base = "px-3 py-1.5 text-sm rounded-md transition-colors font-medium";
  const variants = {
    primary: "bg-autumn-yellow text-sumi-ink1 hover:bg-carp-yellow",
    secondary: "border border-sumi-ink4 text-old-white hover:bg-sumi-ink3",
    ghost: "text-fuji-gray hover:text-old-white hover:bg-sumi-ink3",
    danger:
      "border border-autumn-red/80 text-wave-red hover:bg-winter-red hover:text-peach-red",
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
