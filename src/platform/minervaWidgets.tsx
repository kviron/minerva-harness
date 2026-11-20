import { Children, Fragment, forwardRef, isValidElement, useState, type ReactNode } from "react";
import * as ui from "@/ui";

type SelectChildProps = {
  children?: ReactNode;
  value?: string;
  disabled?: boolean;
  placeholder?: string;
  id?: string;
  "aria-label"?: string;
};
export function SelectItem(_props: SelectChildProps) {
  return null;
}
export function SelectValue(_props: SelectChildProps) {
  return null;
}
export function SelectTrigger(_props: SelectChildProps) {
  return null;
}
export function SelectContent(_props: SelectChildProps) {
  return null;
}
export const SelectGroup = Fragment;
export const SelectLabel = Fragment;

function selectOptions(children: ReactNode, result: ui.SelectOption[] = []): ui.SelectOption[] {
  Children.forEach(children, (child) => {
    if (!isValidElement<SelectChildProps>(child)) return;
    if (child.type === SelectItem && child.props.value) {
      result.push({
        value: child.props.value,
        label: text(child.props.children),
        disabled: child.props.disabled,
        verbatim: true,
      });
    } else selectOptions(child.props.children, result);
  });
  return result;
}

function text(children: ReactNode): string {
  return Children.toArray(children)
    .map((child) =>
      typeof child === "string" || typeof child === "number"
        ? String(child)
        : isValidElement<SelectChildProps>(child)
          ? text(child.props.children)
          : "",
    )
    .join("");
}

function partProps(children: ReactNode, part: unknown): SelectChildProps | undefined {
  let found: SelectChildProps | undefined;
  Children.forEach(children, (child) => {
    if (found || !isValidElement<SelectChildProps>(child)) return;
    found = child.type === part ? child.props : partProps(child.props.children, part);
  });
  return found;
}

/** SDK-v2 compound selects render through the IDE's single Select primitive. */
export function Select({
  value,
  defaultValue,
  onValueChange,
  onChange,
  children,
  options,
  ...props
}: {
  value?: string | null;
  defaultValue?: string;
  onValueChange?: (value: string) => void;
  onChange?: (value: string) => void;
  children?: ReactNode;
  options?: readonly ui.SelectOption[];
  disabled?: boolean;
  "aria-label"?: string;
  placeholder?: string;
  id?: string;
}) {
  const [selected, setSelected] = useState(defaultValue ?? null);
  const trigger = partProps(children, SelectTrigger);
  const display = partProps(children, SelectValue);
  return (
    <ui.Select
      {...props}
      id={props.id ?? trigger?.id}
      aria-label={props["aria-label"] ?? trigger?.["aria-label"]}
      placeholder={props.placeholder ?? display?.placeholder}
      value={value === undefined ? selected : value}
      options={options ?? selectOptions(children)}
      onChange={(next) => {
        setSelected(next);
        onValueChange?.(next);
        onChange?.(next);
      }}
    />
  );
}

type ButtonProps = Omit<ui.ButtonProps, "variant" | "size"> & { variant?: string; size?: string };
/** Legacy SDK names map to host tokens; no legacy CSS or second widget library. */
export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button({ variant, size, ...props }, ref) {
  return (
    <ui.Button
      {...props}
      ref={ref}
      variant={variant === "default" || variant === "primary" ? "primary" : variant === "ghost" ? "ghost" : "secondary"}
      size={size === "sm" || size === "small" ? "small" : "normal"}
    />
  );
});

export const minervaWidgets = {
  ...ui,
  Button,
  Select,
  SelectItem,
  SelectValue,
  SelectTrigger,
  SelectContent,
  SelectGroup,
  SelectLabel,
};
