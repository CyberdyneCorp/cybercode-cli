// Comment
export class Widget extends Base {
  render(props) {
    return <div>{props.title}</div>;
  }
}

function build(options) {
  return new Widget(options);
}

export const handler = async (event) => {
  return build(event);
};
