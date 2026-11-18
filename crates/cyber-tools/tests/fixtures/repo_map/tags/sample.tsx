interface Props {
  title: string;
}

export function Header({ title }: Props) {
  return <h1>{title}</h1>;
}
