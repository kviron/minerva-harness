import { useTx } from "@/platform/i18n";
import { Select, type SelectOption } from "@/ui";

type Props = {
  value: string;
  name: string;
  options: SelectOption[];
  disabled: boolean;
  onChange: (value: string) => void;
};

export function SessionProviderPicker({ value, name, options, disabled, onChange }: Props) {
  const tx = useTx();
  return (
    <Select
      variant="ghost"
      aria-label={tx("Provider")}
      value={value}
      options={options}
      renderValue={() => name}
      disabled={disabled || options.length === 0}
      onChange={onChange}
    />
  );
}
