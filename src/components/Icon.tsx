type Props = {
    name: string;
    size?: number;
    /** Varian terisi, untuk keadaan aktif. */
    fill?: boolean;
    className?: string;
  };

  /** Ikon Material Symbols Rounded. `name` adalah nama ikon di fonts.google.com/icons. */
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
