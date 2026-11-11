type Props = {
    name: string;
    size?: number;
    /** Filled variant for the active state. */
    fill?: boolean;
    className?: string;
  };

  /** Material Symbols Rounded icon. `name` is the icon name from fonts.google.com/icons. */
  export default function Icon({ name, size, fill, className }: Props) {
    return (
      <span
        className={`material-symbols-rounded icon${fill ? " icon--fill" : ""}${
          className ? ` ${className}` : ""
        }`}
        style={size ? { fontSize: size } : undefined}
        aria-hidden="true"
      >
        {name}
      </span>
    );
  }
